pub const PAGE_SIZE: usize = 4096;
pub const POLL_THRESHOLD: usize = 32;
pub const RX_POOL_SIZE: usize = 256;
pub const NET_CTRL_DRIVER_INIT: u32 = 0x4F00;
pub const NET_CTRL_RX_RELEASE: u32 = 0x4F01;
pub const NET_CTRL_MAC_QUERY: u32 = 0x4F02;
pub const NET_CTRL_MAC_REPLY: u32 = 0x4F03;
pub const VIRTIO_NET_F_MRG_RXBUF: u64 = 1u64 << 15;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    Interrupt,
    Poll,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DriverInitMsg {
    pub opcode: u32,
    pub pool_count: u32,
    pub rx_base_iova: u64,
    pub tx_base_iova: u64,
    pub hdr_size: u32,
    pub _pad: u32,
}

const _: () = assert!(core::mem::size_of::<DriverInitMsg>() == 32);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RxReleaseMsg {
    pub opcode: u32,
    pub count: u32,
    pub pool_idx: [u16; 20],
}

const _: () = assert!(core::mem::size_of::<RxReleaseMsg>() == 48);

#[derive(Clone, Copy)]
pub struct NetBufRef {
    pub pool_idx: u16,
    pub len: u16,
}

impl NetBufRef {
    pub const fn empty() -> Self {
        Self {
            pool_idx: 0,
            len: 0,
        }
    }
}

pub struct LocalRing {
    slots: [NetBufRef; 16],
    head: usize,
    tail: usize,
    count: usize,
}

impl LocalRing {
    pub const fn new() -> Self {
        Self {
            slots: [NetBufRef::empty(); 16],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    pub fn push(&mut self, item: NetBufRef) -> bool {
        if self.count == self.slots.len() {
            return false;
        }
        self.slots[self.tail & (self.slots.len() - 1)] = item;
        self.tail = self.tail.wrapping_add(1);
        self.count += 1;
        true
    }

    pub fn pop(&mut self) -> Option<NetBufRef> {
        if self.count == 0 {
            return None;
        }
        let item = self.slots[self.head & (self.slots.len() - 1)];
        self.head = self.head.wrapping_add(1);
        self.count -= 1;
        Some(item)
    }
}

pub struct VirtioNet {
    rx_base_iova: u64,
    tx_base_iova: u64,
    hdr_size: usize,
    mode: TransferMode,
    rx_submitted: [bool; RX_POOL_SIZE],
    rx_to_network: LocalRing,
    tx_from_network: LocalRing,
    dropped_rx: u64,
}

impl VirtioNet {
    pub const fn new() -> Self {
        Self {
            rx_base_iova: 0,
            tx_base_iova: 0,
            hdr_size: 10,
            mode: TransferMode::Interrupt,
            rx_submitted: [false; RX_POOL_SIZE],
            rx_to_network: LocalRing::new(),
            tx_from_network: LocalRing::new(),
            dropped_rx: 0,
        }
    }

    pub fn apply_driver_init(&mut self, init: DriverInitMsg) {
        self.rx_base_iova = init.rx_base_iova;
        self.tx_base_iova = init.tx_base_iova;
        self.hdr_size = init.hdr_size as usize;
        self.populate_rx_descriptors();
    }

    pub fn populate_rx_descriptors(&mut self) {
        for slot in self.rx_submitted.iter_mut() {
            *slot = true;
        }
    }

    pub fn handle_rx_used(&mut self, pool_idx: u16, total_len: u32) -> bool {
        let idx = pool_idx as usize;
        if idx >= RX_POOL_SIZE || !self.rx_submitted[idx] {
            self.dropped_rx = self.dropped_rx.saturating_add(1);
            return false;
        }
        self.rx_submitted[idx] = false;
        let payload_len = total_len
            .saturating_sub(self.hdr_size as u32)
            .min(u16::MAX as u32) as u16;
        if !self.rx_to_network.push(NetBufRef {
            pool_idx,
            len: payload_len,
        }) {
            self.dropped_rx = self.dropped_rx.saturating_add(1);
            self.rx_submitted[idx] = true;
            return false;
        }
        if self.rx_to_network.count >= POLL_THRESHOLD.min(16) {
            self.mode = TransferMode::Poll;
        }
        true
    }

    pub fn process_rx_releases(&mut self, msg: &RxReleaseMsg) -> usize {
        if msg.opcode != NET_CTRL_RX_RELEASE {
            return 0;
        }
        let mut refilled = 0usize;
        for idx in msg.pool_idx.iter().take((msg.count as usize).min(20)) {
            let idx = *idx as usize;
            if idx < RX_POOL_SIZE && !self.rx_submitted[idx] {
                self.rx_submitted[idx] = true;
                refilled += 1;
            }
        }
        refilled
    }

    pub fn queue_tx_from_network(&mut self, pool_idx: u16, len: u16) -> bool {
        self.tx_from_network.push(NetBufRef { pool_idx, len })
    }

    pub fn pop_rx_ready(&mut self) -> Option<NetBufRef> {
        self.rx_to_network.pop()
    }

    pub fn pop_tx_pending(&mut self) -> Option<NetBufRef> {
        self.tx_from_network.pop()
    }

    pub fn flush_tx(&mut self) -> usize {
        let mut flushed = 0usize;
        while self.tx_from_network.pop().is_some() {
            flushed += 1;
        }
        flushed
    }

    pub const fn hdr_size(&self) -> usize {
        self.hdr_size
    }

    pub const fn rx_iova(&self, idx: usize) -> u64 {
        self.rx_base_iova + (idx * PAGE_SIZE) as u64
    }

    pub const fn tx_iova(&self, idx: usize) -> u64 {
        self.tx_base_iova + (idx * PAGE_SIZE) as u64
    }
}

pub const fn negotiate_hdr_size(features: u64) -> usize {
    if features & VIRTIO_NET_F_MRG_RXBUF != 0 {
        12
    } else {
        12
    }
}
