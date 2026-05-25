#![no_std]
#![no_main]
#![allow(dead_code, static_mut_refs)]

use core::panic::PanicInfo;

use exo_syscall_abi as syscall;

mod config;
mod interrupt;
mod mac;
mod net;
mod pci;
mod virtqueue;

use config::{
    PAGE_SIZE, VIRTIO_F_VERSION_1, VIRTIO_NET_F_MAC, VIRTIO_PCI_COMMON_DEVICE_FEATURE,
    VIRTIO_PCI_COMMON_DEVICE_FEATURE_SELECT, VIRTIO_PCI_COMMON_DEVICE_STATUS,
    VIRTIO_PCI_COMMON_DRIVER_FEATURE, VIRTIO_PCI_COMMON_DRIVER_FEATURE_SELECT,
    VIRTIO_PCI_COMMON_QUEUE_DESC, VIRTIO_PCI_COMMON_QUEUE_DEVICE, VIRTIO_PCI_COMMON_QUEUE_DRIVER,
    VIRTIO_PCI_COMMON_QUEUE_ENABLE, VIRTIO_PCI_COMMON_QUEUE_NOTIFY_OFF,
    VIRTIO_PCI_COMMON_QUEUE_SELECT, VIRTIO_PCI_COMMON_QUEUE_SIZE, VIRTIO_STATUS_ACKNOWLEDGE,
    VIRTIO_STATUS_DRIVER, VIRTIO_STATUS_DRIVER_OK, VIRTIO_STATUS_FAILED, VIRTIO_STATUS_FEATURES_OK,
    VRING_QUEUE_SIZE,
};
use virtqueue::{Virtqueue, VIRTQ_DESC_F_WRITE};

const SERVER_ENDPOINT_ID: u64 = 14;
const NETWORK_ENDPOINT_ID: u64 = 7;
const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;
const INVALID_POOL: u16 = u16::MAX;
const NET_CTRL_RX_READY: u32 = 0x4F04;
const NET_CTRL_TX_SUBMIT: u32 = 0x4F05;
const NET_CTRL_TX_COMPLETE: u32 = 0x4F06;
const IPC_RECV_TIMEOUT_MS: u64 = 2;

#[repr(C)]
struct DriverRequest {
    sender_pid: u32,
    msg_type: u32,
    payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
}

impl DriverRequest {
    const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RxPacketRef {
    pool_idx: u16,
    len: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RxReadyMsg {
    opcode: u32,
    count: u32,
    entries: [RxPacketRef; 20],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TxSubmitMsg {
    opcode: u32,
    pool_idx: u16,
    len: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TxCompleteMsg {
    opcode: u32,
    count: u32,
    pool_idx: [u16; 20],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MacReplyMsg {
    opcode: u32,
    mac: [u8; 6],
    _pad: [u8; 2],
}

struct VirtioHardware {
    common_cfg: *mut u8,
    notify_cfg: *mut u8,
    notify_off_multiplier: u32,
    isr_cfg: *mut u8,
    device_cfg: *mut u8,
    bdf_raw: u32,
    irq_line: u8,
    irq_reg_id: u64,
    negotiated_features: u64,
    mac: [u8; 6],
    rx_queue: Virtqueue,
    tx_queue: Virtqueue,
    rx_notify: *mut u8,
    tx_notify: *mut u8,
    rx_pool_for_head: [u16; 256],
    tx_pool_for_head: [u16; 256],
    rx_base_iova: u64,
    tx_base_iova: u64,
    hdr_size: usize,
    pool_count: usize,
    online: bool,
    pool_ready: bool,
    saw_tx: bool,
    saw_tx_complete: bool,
    saw_rx: bool,
}

impl VirtioHardware {
    const fn new() -> Self {
        Self {
            common_cfg: core::ptr::null_mut(),
            notify_cfg: core::ptr::null_mut(),
            notify_off_multiplier: 0,
            isr_cfg: core::ptr::null_mut(),
            device_cfg: core::ptr::null_mut(),
            bdf_raw: 0,
            irq_line: 0,
            irq_reg_id: 0,
            negotiated_features: 0,
            mac: mac::DEFAULT_MAC,
            rx_queue: Virtqueue::empty(),
            tx_queue: Virtqueue::empty(),
            rx_notify: core::ptr::null_mut(),
            tx_notify: core::ptr::null_mut(),
            rx_pool_for_head: [INVALID_POOL; 256],
            tx_pool_for_head: [INVALID_POOL; 256],
            rx_base_iova: 0,
            tx_base_iova: 0,
            hdr_size: config::VIRTIO_NET_HDR_SIZE_MODERN,
            pool_count: 0,
            online: false,
            pool_ready: false,
            saw_tx: false,
            saw_tx_complete: false,
            saw_rx: false,
        }
    }

    fn init_hardware(&mut self) -> Result<(), i64> {
        let dev = pci::discover_and_map()?;
        self.common_cfg = dev.common_cfg;
        self.notify_cfg = dev.notify_cfg;
        self.notify_off_multiplier = dev.notify_off_multiplier;
        self.isr_cfg = dev.isr_cfg;
        self.device_cfg = dev.device_cfg;
        self.bdf_raw = dev.bdf_raw;
        self.irq_line = dev.irq_line;
        debug_write(b"virtio_net_driver: pci capabilities ready\n");

        unsafe {
            write8(self.common_cfg, VIRTIO_PCI_COMMON_DEVICE_STATUS, 0);
            self.set_status(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

            let offered = self.read_features();
            if offered & VIRTIO_F_VERSION_1 == 0 {
                self.fail_device();
                return Err(syscall::ENODEV);
            }
            self.negotiated_features = offered & (VIRTIO_F_VERSION_1 | VIRTIO_NET_F_MAC);
            self.write_features(self.negotiated_features);
            self.set_status(
                VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
            );
            if read8(self.common_cfg, VIRTIO_PCI_COMMON_DEVICE_STATUS)
                & VIRTIO_STATUS_FEATURES_OK as u8
                == 0
            {
                self.fail_device();
                return Err(syscall::ENODEV);
            }

            self.rx_queue = Virtqueue::init(VRING_QUEUE_SIZE).map_err(|_| syscall::ENOMEM)?;
            self.tx_queue = Virtqueue::init(VRING_QUEUE_SIZE).map_err(|_| syscall::ENOMEM)?;
            self.rx_notify = self.setup_queue(RX_QUEUE, &self.rx_queue)?;
            self.tx_notify = self.setup_queue(TX_QUEUE, &self.tx_queue)?;
            self.mac = mac::read_mac(self.device_cfg, self.negotiated_features);
            self.irq_reg_id =
                interrupt::register_irq(self.irq_line, SERVER_ENDPOINT_ID, self.bdf_raw)
                    .unwrap_or(0);
            self.set_status(
                VIRTIO_STATUS_ACKNOWLEDGE
                    | VIRTIO_STATUS_DRIVER
                    | VIRTIO_STATUS_FEATURES_OK
                    | VIRTIO_STATUS_DRIVER_OK,
            );
        }

        self.online = true;
        Ok(())
    }

    unsafe fn set_status(&self, status: u32) {
        unsafe {
            write8(
                self.common_cfg,
                VIRTIO_PCI_COMMON_DEVICE_STATUS,
                status as u8,
            )
        };
    }

    unsafe fn fail_device(&self) {
        unsafe {
            write8(
                self.common_cfg,
                VIRTIO_PCI_COMMON_DEVICE_STATUS,
                VIRTIO_STATUS_FAILED as u8,
            )
        };
    }

    unsafe fn read_features(&self) -> u64 {
        unsafe {
            write32(self.common_cfg, VIRTIO_PCI_COMMON_DEVICE_FEATURE_SELECT, 0);
            let low = read32(self.common_cfg, VIRTIO_PCI_COMMON_DEVICE_FEATURE) as u64;
            write32(self.common_cfg, VIRTIO_PCI_COMMON_DEVICE_FEATURE_SELECT, 1);
            let high = read32(self.common_cfg, VIRTIO_PCI_COMMON_DEVICE_FEATURE) as u64;
            (high << 32) | low
        }
    }

    unsafe fn write_features(&self, features: u64) {
        unsafe {
            write32(self.common_cfg, VIRTIO_PCI_COMMON_DRIVER_FEATURE_SELECT, 0);
            write32(
                self.common_cfg,
                VIRTIO_PCI_COMMON_DRIVER_FEATURE,
                features as u32,
            );
            write32(self.common_cfg, VIRTIO_PCI_COMMON_DRIVER_FEATURE_SELECT, 1);
            write32(
                self.common_cfg,
                VIRTIO_PCI_COMMON_DRIVER_FEATURE,
                (features >> 32) as u32,
            );
        }
    }

    unsafe fn setup_queue(&self, queue_idx: u16, queue: &Virtqueue) -> Result<*mut u8, i64> {
        unsafe {
            write16(self.common_cfg, VIRTIO_PCI_COMMON_QUEUE_SELECT, queue_idx);
            let max = read16(self.common_cfg, VIRTIO_PCI_COMMON_QUEUE_SIZE);
            if max == 0 || max < queue.queue_size {
                return Err(syscall::ENODEV);
            }
            write16(
                self.common_cfg,
                VIRTIO_PCI_COMMON_QUEUE_SIZE,
                queue.queue_size,
            );
            write64(
                self.common_cfg,
                VIRTIO_PCI_COMMON_QUEUE_DESC,
                queue.phys_base,
            );
            write64(
                self.common_cfg,
                VIRTIO_PCI_COMMON_QUEUE_DRIVER,
                queue.avail_phys(),
            );
            write64(
                self.common_cfg,
                VIRTIO_PCI_COMMON_QUEUE_DEVICE,
                queue.used_phys(),
            );
            let notify_off = read16(self.common_cfg, VIRTIO_PCI_COMMON_QUEUE_NOTIFY_OFF);
            let notify_addr = (notify_off as usize)
                .checked_mul(self.notify_off_multiplier as usize)
                .ok_or(syscall::ENODEV)?;
            write16(self.common_cfg, VIRTIO_PCI_COMMON_QUEUE_ENABLE, 1);
            Ok(self.notify_cfg.add(notify_addr))
        }
    }

    fn apply_driver_init(&mut self, init: net::DriverInitMsg, state: &mut net::VirtioNet) {
        if init.opcode != net::NET_CTRL_DRIVER_INIT {
            return;
        }
        state.apply_driver_init(init);
        self.rx_base_iova = init.rx_base_iova;
        self.tx_base_iova = init.tx_base_iova;
        self.hdr_size = init.hdr_size as usize;
        self.pool_count = (init.pool_count as usize).min(self.rx_pool_for_head.len());
        self.pool_ready = self.online && self.pool_count != 0;
        if self.pool_ready {
            debug_write(b"virtio_net_driver: pool ready\n");
            let mut idx = 0usize;
            while idx < self.pool_count {
                let _ = self.submit_rx_slot(idx as u16);
                idx += 1;
            }
            unsafe { Virtqueue::notify(self.rx_notify, RX_QUEUE) };
        }
        self.send_mac_reply();
    }

    fn process_rx_releases(&mut self, msg: &net::RxReleaseMsg, state: &mut net::VirtioNet) {
        let count = state.process_rx_releases(msg);
        if !self.pool_ready || count == 0 {
            return;
        }
        let mut i = 0usize;
        while i < (msg.count as usize).min(msg.pool_idx.len()) {
            let pool_idx = msg.pool_idx[i];
            if (pool_idx as usize) < self.pool_count {
                let _ = self.submit_rx_slot(pool_idx);
            }
            i += 1;
        }
        unsafe { Virtqueue::notify(self.rx_notify, RX_QUEUE) };
    }

    fn submit_rx_slot(&mut self, pool_idx: u16) -> Result<(), i64> {
        if !self.pool_ready && !self.online {
            return Err(syscall::ENODEV);
        }
        let addr = self.rx_base_iova + (pool_idx as usize * PAGE_SIZE) as u64;
        let bufs = [(addr, PAGE_SIZE as u32, VIRTQ_DESC_F_WRITE)];
        let head = unsafe {
            self.rx_queue
                .add_chain(&bufs)
                .map_err(|_| syscall::ENOBUFS)?
        };
        self.rx_pool_for_head[head as usize] = pool_idx;
        Ok(())
    }

    fn submit_tx(&mut self, pool_idx: u16, len: u16) -> Result<(), i64> {
        if !self.online || self.tx_base_iova == 0 {
            return Err(syscall::ENODEV);
        }
        let total_len = (len as usize)
            .saturating_add(self.hdr_size)
            .min(PAGE_SIZE)
            .max(self.hdr_size);
        let addr = self.tx_base_iova + (pool_idx as usize * PAGE_SIZE) as u64;
        let bufs = [(addr, total_len as u32, 0)];
        unsafe {
            let head = self
                .tx_queue
                .add_chain(&bufs)
                .map_err(|_| syscall::ENOBUFS)?;
            self.tx_pool_for_head[head as usize] = pool_idx;
            if !self.saw_tx {
                self.saw_tx = true;
                debug_write(b"virtio_net_driver: first tx\n");
            }
            Virtqueue::notify(self.tx_notify, TX_QUEUE);
        }
        Ok(())
    }

    fn poll(&mut self, state: &mut net::VirtioNet) {
        if !self.online {
            return;
        }
        unsafe {
            let _ = interrupt::ack_pending(self.isr_cfg);
        }
        self.poll_rx(state);
        self.poll_tx();
    }

    fn poll_rx(&mut self, state: &mut net::VirtioNet) {
        if !self.pool_ready {
            return;
        }
        let mut ready = RxReadyMsg {
            opcode: NET_CTRL_RX_READY,
            count: 0,
            entries: [RxPacketRef {
                pool_idx: 0,
                len: 0,
            }; 20],
        };

        loop {
            let Some((head, total_len)) = (unsafe { self.rx_queue.poll_used() }) else {
                break;
            };
            let pool_idx = self.rx_pool_for_head[head as usize];
            self.rx_pool_for_head[head as usize] = INVALID_POOL;
            unsafe { self.rx_queue.recycle_desc(head) };
            if pool_idx != INVALID_POOL && state.handle_rx_used(pool_idx, total_len) {
                if !self.saw_rx {
                    self.saw_rx = true;
                    debug_write(b"virtio_net_driver: first rx\n");
                }
                while let Some(pkt) = state.pop_rx_ready() {
                    if ready.count as usize == ready.entries.len() {
                        send_rx_ready(&ready);
                        ready.count = 0;
                    }
                    ready.entries[ready.count as usize] = RxPacketRef {
                        pool_idx: pkt.pool_idx,
                        len: pkt.len,
                    };
                    ready.count += 1;
                }
            }
        }

        if ready.count != 0 {
            send_rx_ready(&ready);
        }
    }

    fn poll_tx(&mut self) {
        if !self.online {
            return;
        }
        let mut complete = TxCompleteMsg {
            opcode: NET_CTRL_TX_COMPLETE,
            count: 0,
            pool_idx: [0; 20],
        };
        while let Some((head, _len)) = unsafe { self.tx_queue.poll_used() } {
            let pool_idx = if (head as usize) < self.tx_pool_for_head.len() {
                let pool_idx = self.tx_pool_for_head[head as usize];
                self.tx_pool_for_head[head as usize] = INVALID_POOL;
                pool_idx
            } else {
                INVALID_POOL
            };
            unsafe { self.tx_queue.recycle_desc(head) };
            if pool_idx != INVALID_POOL {
                if !self.saw_tx_complete {
                    self.saw_tx_complete = true;
                    debug_write(b"virtio_net_driver: first tx complete\n");
                }
                complete.pool_idx[complete.count as usize] = pool_idx;
                complete.count += 1;
                if complete.count as usize == complete.pool_idx.len() {
                    send_tx_complete(&complete);
                    complete.count = 0;
                }
            }
        }
        if complete.count != 0 {
            send_tx_complete(&complete);
        }
    }

    fn send_mac_reply(&self) {
        let msg = MacReplyMsg {
            opcode: net::NET_CTRL_MAC_REPLY,
            mac: self.mac,
            _pad: [0; 2],
        };
        let payload = unsafe {
            core::slice::from_raw_parts(
                &msg as *const MacReplyMsg as *const u8,
                core::mem::size_of::<MacReplyMsg>(),
            )
        };
        send_ctrl(NETWORK_ENDPOINT_ID, net::NET_CTRL_MAC_REPLY, payload);
    }
}

static mut VIRTIO_NET: net::VirtioNet = net::VirtioNet::new();
static mut VIRTIO_HW: VirtioHardware = VirtioHardware::new();

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_service_endpoint();
    debug_write(b"virtio_net_driver: boot\n");
    unsafe {
        match VIRTIO_HW.init_hardware() {
            Ok(()) => {
                register_transport_endpoint();
                debug_write(b"virtio_net_driver: hardware ready\n");
            }
            Err(err) if err == syscall::ENOENT => {
                debug_write(b"virtio_net_driver: hardware absent\n");
            }
            Err(err) => debug_errno(b"virtio_net_driver: hardware unavailable errno ", err),
        }
    }

    let mut request = DriverRequest::zeroed();
    loop {
        let rc = recv(&mut request);
        unsafe {
            if rc > 0 {
                match request.msg_type {
                    net::NET_CTRL_DRIVER_INIT => {
                        let init = core::ptr::read_unaligned(
                            request.payload.as_ptr() as *const net::DriverInitMsg
                        );
                        VIRTIO_HW.apply_driver_init(init, &mut VIRTIO_NET);
                    }
                    net::NET_CTRL_RX_RELEASE => {
                        let msg = core::ptr::read_unaligned(
                            request.payload.as_ptr() as *const net::RxReleaseMsg
                        );
                        VIRTIO_HW.process_rx_releases(&msg, &mut VIRTIO_NET);
                    }
                    net::NET_CTRL_MAC_QUERY => VIRTIO_HW.send_mac_reply(),
                    NET_CTRL_TX_SUBMIT => {
                        let msg = core::ptr::read_unaligned(
                            request.payload.as_ptr() as *const TxSubmitMsg
                        );
                        if msg.opcode == NET_CTRL_TX_SUBMIT {
                            let _ = VIRTIO_NET.queue_tx_from_network(msg.pool_idx, msg.len);
                            while let Some(tx) = VIRTIO_NET.pop_tx_pending() {
                                if VIRTIO_HW.submit_tx(tx.pool_idx, tx.len).is_err() {
                                    send_single_tx_complete(tx.pool_idx);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            VIRTIO_HW.poll(&mut VIRTIO_NET);
        }
    }
}

fn register_service_endpoint() {
    register_endpoint_name(b"virtio_net_driver");
}

fn register_transport_endpoint() {
    register_endpoint_name(b"virtio_net");
}

fn register_endpoint_name(name: &[u8]) {
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            SERVER_ENDPOINT_ID,
        );
    }
}

fn recv(request: &mut DriverRequest) -> i64 {
    unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            SERVER_ENDPOINT_ID,
            request as *mut DriverRequest as u64,
            core::mem::size_of::<DriverRequest>() as u64,
            syscall::IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
        )
    }
}

fn send_ctrl(endpoint: u64, msg_type: u32, payload: &[u8]) {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) }.max(0) as u32;
    let mut msg = DriverRequest {
        sender_pid: pid,
        msg_type,
        payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
    };
    let n = payload.len().min(msg.payload.len());
    msg.payload[..n].copy_from_slice(&payload[..n]);
    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            endpoint,
            &msg as *const DriverRequest as u64,
            core::mem::size_of::<DriverRequest>() as u64,
            syscall::IPC_FLAG_INJECT_SRC_PID,
            0,
            0,
        )
    };
}

fn send_rx_ready(ready: &RxReadyMsg) {
    let payload = unsafe {
        core::slice::from_raw_parts(
            ready as *const RxReadyMsg as *const u8,
            core::mem::size_of::<RxReadyMsg>(),
        )
    };
    send_ctrl(NETWORK_ENDPOINT_ID, NET_CTRL_RX_READY, payload);
}

fn send_tx_complete(complete: &TxCompleteMsg) {
    let payload = unsafe {
        core::slice::from_raw_parts(
            complete as *const TxCompleteMsg as *const u8,
            core::mem::size_of::<TxCompleteMsg>(),
        )
    };
    send_ctrl(NETWORK_ENDPOINT_ID, NET_CTRL_TX_COMPLETE, payload);
}

fn send_single_tx_complete(pool_idx: u16) {
    let complete = TxCompleteMsg {
        opcode: NET_CTRL_TX_COMPLETE,
        count: 1,
        pool_idx: {
            let mut slots = [0; 20];
            slots[0] = pool_idx;
            slots
        },
    };
    send_tx_complete(&complete);
}

#[inline]
unsafe fn read8(base: *mut u8, reg: usize) -> u8 {
    unsafe { core::ptr::read_volatile(base.add(reg) as *const u8) }
}

#[inline]
unsafe fn read16(base: *mut u8, reg: usize) -> u16 {
    unsafe { core::ptr::read_volatile(base.add(reg) as *const u16) }
}

#[inline]
unsafe fn read32(base: *mut u8, reg: usize) -> u32 {
    unsafe { core::ptr::read_volatile(base.add(reg) as *const u32) }
}

#[inline]
unsafe fn write8(base: *mut u8, reg: usize, value: u8) {
    unsafe { core::ptr::write_volatile(base.add(reg) as *mut u8, value) };
}

#[inline]
unsafe fn write16(base: *mut u8, reg: usize, value: u16) {
    unsafe { core::ptr::write_volatile(base.add(reg) as *mut u16, value) };
}

#[inline]
unsafe fn write32(base: *mut u8, reg: usize, value: u32) {
    unsafe { core::ptr::write_volatile(base.add(reg) as *mut u32, value) };
}

#[inline]
unsafe fn write64(base: *mut u8, reg: usize, value: u64) {
    unsafe { core::ptr::write_volatile(base.add(reg) as *mut u64, value) };
}

fn debug_write(bytes: &[u8]) {
    for &byte in bytes {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("out 0xE9, al", in("al") byte, options(nomem, nostack));
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = byte;
    }
}

fn debug_errno(prefix: &[u8], err: i64) {
    debug_write(prefix);
    let negative = err < 0;
    let mut value = if negative {
        err.wrapping_neg() as u64
    } else {
        err as u64
    };
    if negative {
        debug_write(b"-");
    }
    let mut digits = [0u8; 20];
    let mut pos = digits.len();
    if value == 0 {
        pos -= 1;
        digits[pos] = b'0';
    } else {
        while value != 0 {
            pos -= 1;
            digits[pos] = b'0' + (value % 10) as u8;
            value /= 10;
        }
    }
    debug_write(&digits[pos..]);
    debug_write(b"\n");
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
