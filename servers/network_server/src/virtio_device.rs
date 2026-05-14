use crate::buf_pool::{NetBufPool, RX_POOL_SIZE};

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

pub struct PacketRing {
    slots: [NetBufRef; 16],
    head: usize,
    tail: usize,
    count: usize,
}

impl PacketRing {
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

    pub const fn len(&self) -> usize {
        self.count
    }
}

pub struct ExoNetDevice {
    rx_ring: PacketRing,
    tx_ring: PacketRing,
    pub released_buf: [u16; 64],
    pub released_count: usize,
    pub dropped_rx: u64,
    pub dropped_tx: u64,
}

impl ExoNetDevice {
    pub const fn new() -> Self {
        Self {
            rx_ring: PacketRing::new(),
            tx_ring: PacketRing::new(),
            released_buf: [0; 64],
            released_count: 0,
            dropped_rx: 0,
            dropped_tx: 0,
        }
    }

    pub fn push_rx_from_driver(&mut self, pool_idx: u16, len: u16) -> bool {
        if (pool_idx as usize) >= RX_POOL_SIZE {
            self.dropped_rx = self.dropped_rx.saturating_add(1);
            return false;
        }
        if self.rx_ring.push(NetBufRef { pool_idx, len }) {
            true
        } else {
            self.dropped_rx = self.dropped_rx.saturating_add(1);
            self.release_rx(pool_idx);
            false
        }
    }

    pub fn poll_ingress_single(&mut self, _pool: &NetBufPool) -> bool {
        let Some(buf) = self.rx_ring.pop() else {
            return false;
        };
        self.release_rx(buf.pool_idx);
        true
    }

    pub fn submit_tx(&mut self, pool: &NetBufPool, len: usize) -> Result<u16, i64> {
        let Some(idx) = pool.tx_alloc() else {
            self.dropped_tx = self.dropped_tx.saturating_add(1);
            return Err(exo_syscall_abi::ENOBUFS);
        };
        if pool.ready() {
            unsafe {
                core::ptr::write_bytes(pool.tx_header_ptr_mut(idx as usize), 0, pool.hdr_size());
            }
        }
        if !self.tx_ring.push(NetBufRef {
            pool_idx: idx,
            len: len.min(u16::MAX as usize) as u16,
        }) {
            pool.tx_free(idx);
            self.dropped_tx = self.dropped_tx.saturating_add(1);
            return Err(exo_syscall_abi::ENOBUFS);
        }
        Ok(idx)
    }

    pub fn pop_tx_for_driver(&mut self) -> Option<NetBufRef> {
        self.tx_ring.pop()
    }

    pub fn release_rx(&mut self, pool_idx: u16) {
        if self.released_count < self.released_buf.len() {
            self.released_buf[self.released_count] = pool_idx;
            self.released_count += 1;
        } else {
            self.dropped_rx = self.dropped_rx.saturating_add(1);
        }
    }

    pub fn rx_depth(&self) -> usize {
        self.rx_ring.len()
    }
}
