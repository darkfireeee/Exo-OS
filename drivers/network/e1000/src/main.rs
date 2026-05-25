#![no_std]
#![no_main]
#![allow(dead_code, static_mut_refs)]

use core::panic::PanicInfo;

use exo_syscall_abi as syscall;

mod interrupt;
mod mac;
mod pci;
mod regs;
mod rx;
mod tx;

use rx::{RxRing, RX_RING_SIZE};
use tx::{TxRing, TX_RING_SIZE};

const ENDPOINT_ID: u64 = 16;
const NETWORK_ENDPOINT_ID: u64 = 7;
const PAGE_SIZE: usize = 4096;
const DMA_DIR_BIDIR: u64 = 2;
// e1000 descriptors need DMA-visible physical addresses until the kernel
// programs a translated IOMMU context for this Ring1 driver.
const DMA_MAP_FLAGS_BYPASS_IOMMU: u64 = 1 << 4;

const NET_CTRL_DRIVER_INIT: u32 = 0x4F00;
const NET_CTRL_RX_RELEASE: u32 = 0x4F01;
const NET_CTRL_MAC_QUERY: u32 = 0x4F02;
const NET_CTRL_MAC_REPLY: u32 = 0x4F03;
const NET_CTRL_RX_READY: u32 = 0x4F04;
const NET_CTRL_TX_SUBMIT: u32 = 0x4F05;
const NET_CTRL_TX_COMPLETE: u32 = 0x4F06;
const IPC_RECV_TIMEOUT_MS: u64 = 2;

#[repr(C)]
struct NetCtrlEnvelope {
    sender_pid: u32,
    msg_type: u32,
    payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
}

impl NetCtrlEnvelope {
    const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

const _: () = assert!(core::mem::size_of::<NetCtrlEnvelope>() == syscall::IPC_ENVELOPE_SIZE);
const _: () = assert!(core::mem::offset_of!(NetCtrlEnvelope, payload) == syscall::IPC_HEADER_SIZE);

#[repr(C)]
#[derive(Clone, Copy)]
struct DriverInitMsg {
    opcode: u32,
    pool_count: u32,
    rx_base_iova: u64,
    tx_base_iova: u64,
    hdr_size: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RxReleaseMsg {
    opcode: u32,
    count: u32,
    pool_idx: [u16; 20],
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
struct RxPacketRef {
    pool_idx: u16,
    len: u16,
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

struct DmaRegion {
    virt: u64,
    iova: u64,
}

struct E1000Driver {
    mmio: *mut u8,
    bdf_raw: u32,
    irq_line: u8,
    irq_reg_id: u64,
    mac: [u8; 6],
    rx: RxRing,
    tx: TxRing,
    rx_base_iova: u64,
    tx_base_iova: u64,
    hdr_size: usize,
    pool_count: usize,
    online: bool,
    pool_ready: bool,
    saw_tx: bool,
    saw_rx: bool,
}

impl E1000Driver {
    const fn new() -> Self {
        Self {
            mmio: core::ptr::null_mut(),
            bdf_raw: 0,
            irq_line: 0,
            irq_reg_id: 0,
            mac: mac::DEFAULT_MAC,
            rx: RxRing::empty(),
            tx: TxRing::empty(),
            rx_base_iova: 0,
            tx_base_iova: 0,
            hdr_size: 10,
            pool_count: 0,
            online: false,
            pool_ready: false,
            saw_tx: false,
            saw_rx: false,
        }
    }

    fn init_hardware(&mut self) -> Result<(), i64> {
        let pci = pci::discover_and_map()?;
        self.mmio = pci.bar0_virt;
        self.bdf_raw = pci.bdf_raw;
        self.irq_line = pci.irq_line;
        debug_write(b"e1000_driver: pci mapped\n");

        unsafe {
            self.reset_device()?;
        }
        let rx_mem = dma_alloc(core::mem::size_of::<rx::RxDesc>() * RX_RING_SIZE)?;
        let tx_mem = dma_alloc(core::mem::size_of::<tx::TxDesc>() * TX_RING_SIZE)?;
        debug_write(b"e1000_driver: rings mapped\n");

        unsafe {
            interrupt::disable(self.mmio);
            self.rx.init(rx_mem.virt, rx_mem.iova);
            self.tx.init(tx_mem.virt, tx_mem.iova);
            debug_write(b"e1000_driver: rings initialized\n");
            self.program_rings();
            self.mac = mac::read_mac(self.mmio);
            self.program_tx();
            self.clear_multicast_table();
            self.irq_reg_id =
                interrupt::register_irq(self.irq_line, ENDPOINT_ID, self.bdf_raw).unwrap_or(0);
        }
        self.online = true;
        Ok(())
    }

    unsafe fn reset_device(&self) -> Result<(), i64> {
        let ctrl = unsafe { read32(self.mmio, regs::CTRL) };
        unsafe { write32(self.mmio, regs::CTRL, ctrl | regs::CTRL_RST) };

        let mut spins = 0usize;
        while spins < 1_000_000 {
            if unsafe { read32(self.mmio, regs::CTRL) } & regs::CTRL_RST == 0 {
                let ctrl = unsafe { read32(self.mmio, regs::CTRL) };
                unsafe {
                    write32(self.mmio, regs::CTRL, ctrl | regs::CTRL_SLU);
                    interrupt::disable(self.mmio);
                    let _ = interrupt::read_cause(self.mmio);
                }
                debug_write(b"e1000_driver: reset ready\n");
                return Ok(());
            }
            core::hint::spin_loop();
            spins += 1;
        }
        Err(syscall::EAGAIN)
    }

    unsafe fn program_rings(&self) {
        unsafe {
            write32(self.mmio, regs::RDBAL, self.rx.iova as u32);
            write32(self.mmio, regs::RDBAH, (self.rx.iova >> 32) as u32);
            write32(
                self.mmio,
                regs::RDLEN,
                (core::mem::size_of::<rx::RxDesc>() * RX_RING_SIZE) as u32,
            );
            write32(self.mmio, regs::RDH, 0);
            write32(self.mmio, regs::RDT, 0);

            write32(self.mmio, regs::TDBAL, self.tx.iova as u32);
            write32(self.mmio, regs::TDBAH, (self.tx.iova >> 32) as u32);
            write32(
                self.mmio,
                regs::TDLEN,
                (core::mem::size_of::<tx::TxDesc>() * TX_RING_SIZE) as u32,
            );
            write32(self.mmio, regs::TDH, 0);
            write32(self.mmio, regs::TDT, 0);
        }
    }

    unsafe fn program_tx(&self) {
        let tctl = regs::TCTL_EN
            | regs::TCTL_PSP
            | (0x0f << regs::TCTL_CT_SHIFT)
            | (0x40 << regs::TCTL_COLD_SHIFT);
        unsafe {
            write32(self.mmio, regs::TCTL, tctl);
            write32(self.mmio, regs::TIPG, 0x0060_2006);
        }
    }

    unsafe fn program_rx(&self) {
        let rctl = regs::RCTL_EN
            | regs::RCTL_SBP
            | regs::RCTL_UPE
            | regs::RCTL_MPE
            | regs::RCTL_BAM
            | regs::RCTL_BSIZE_2048
            | regs::RCTL_SECRC;
        unsafe { write32(self.mmio, regs::RCTL, rctl) };
    }

    unsafe fn clear_multicast_table(&self) {
        let mut idx = 0usize;
        while idx < 128 {
            unsafe { write32(self.mmio, regs::MTA + idx * 4, 0) };
            idx += 1;
        }
    }

    fn apply_driver_init(&mut self, init: DriverInitMsg) {
        if init.opcode != NET_CTRL_DRIVER_INIT || !self.online {
            return;
        }
        self.rx_base_iova = init.rx_base_iova;
        self.tx_base_iova = init.tx_base_iova;
        self.hdr_size = init.hdr_size as usize;
        self.pool_count = (init.pool_count as usize).min(RX_RING_SIZE);
        let mut idx = 0usize;
        while idx < self.pool_count {
            unsafe {
                self.rx
                    .set_buffer(idx, self.rx_iova(idx).saturating_add(self.hdr_size as u64));
            }
            idx += 1;
        }
        self.pool_ready = true;
        debug_write(b"e1000_driver: pool ready\n");
        unsafe {
            write32(
                self.mmio,
                regs::RDT,
                self.pool_count.saturating_sub(1) as u32,
            );
            self.program_rx();
            interrupt::enable_basic(self.mmio);
        }
        self.send_mac_reply();
    }

    fn process_rx_releases(&mut self, msg: RxReleaseMsg) {
        if msg.opcode != NET_CTRL_RX_RELEASE || !self.online || !self.pool_ready {
            return;
        }
        let count = (msg.count as usize).min(msg.pool_idx.len());
        let mut i = 0usize;
        while i < count {
            let idx = msg.pool_idx[i] as usize;
            if idx < self.pool_count {
                unsafe {
                    self.rx
                        .set_buffer(idx, self.rx_iova(idx).saturating_add(self.hdr_size as u64));
                    write32(self.mmio, regs::RDT, idx as u32);
                }
            }
            i += 1;
        }
    }

    fn process_tx_submit(&mut self, msg: TxSubmitMsg) {
        if msg.opcode != NET_CTRL_TX_SUBMIT {
            return;
        }
        let idx = msg.pool_idx as usize;
        if !self.online || !self.pool_ready || idx >= TX_RING_SIZE || msg.len == 0 {
            send_single_tx_complete(msg.pool_idx);
            return;
        }
        let addr = self.tx_iova(idx).saturating_add(self.hdr_size as u64);
        unsafe {
            if let Some(desc_idx) = self.tx.prepare(addr, msg.len) {
                if !self.saw_tx {
                    self.saw_tx = true;
                    debug_write(b"e1000_driver: first tx\n");
                }
                write32(self.mmio, regs::TDT, self.tx.tail as u32);
                let mut spins = 0usize;
                while spins < 100_000 {
                    if self.tx.completed(desc_idx) {
                        break;
                    }
                    core::hint::spin_loop();
                    spins += 1;
                }
            }
        }
        send_single_tx_complete(msg.pool_idx);
    }

    fn poll_rx(&mut self) {
        if !self.online || !self.pool_ready {
            return;
        }
        unsafe {
            let _ = interrupt::read_cause(self.mmio);
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
            let Some((pool_idx, len)) = (unsafe { self.rx.poll_one() }) else {
                break;
            };
            if !self.saw_rx {
                self.saw_rx = true;
                debug_write(b"e1000_driver: first rx\n");
            }
            let slot = ready.count as usize;
            if slot == ready.entries.len() {
                self.send_rx_ready(&ready);
                ready.count = 0;
            }
            ready.entries[ready.count as usize] = RxPacketRef { pool_idx, len };
            ready.count += 1;
        }
        if ready.count != 0 {
            self.send_rx_ready(&ready);
        }
    }

    fn send_rx_ready(&self, ready: &RxReadyMsg) {
        let payload = unsafe {
            core::slice::from_raw_parts(
                ready as *const RxReadyMsg as *const u8,
                core::mem::size_of::<RxReadyMsg>(),
            )
        };
        send_ctrl(NETWORK_ENDPOINT_ID, NET_CTRL_RX_READY, payload);
    }

    fn send_tx_complete(&self, complete: &TxCompleteMsg) {
        let payload = unsafe {
            core::slice::from_raw_parts(
                complete as *const TxCompleteMsg as *const u8,
                core::mem::size_of::<TxCompleteMsg>(),
            )
        };
        send_ctrl(NETWORK_ENDPOINT_ID, NET_CTRL_TX_COMPLETE, payload);
    }

    fn send_mac_reply(&self) {
        let msg = MacReplyMsg {
            opcode: NET_CTRL_MAC_REPLY,
            mac: self.mac,
            _pad: [0; 2],
        };
        let payload = unsafe {
            core::slice::from_raw_parts(
                &msg as *const MacReplyMsg as *const u8,
                core::mem::size_of::<MacReplyMsg>(),
            )
        };
        send_ctrl(NETWORK_ENDPOINT_ID, NET_CTRL_MAC_REPLY, payload);
    }

    #[inline]
    fn rx_iova(&self, idx: usize) -> u64 {
        self.rx_base_iova + (idx * PAGE_SIZE) as u64
    }

    #[inline]
    fn tx_iova(&self, idx: usize) -> u64 {
        self.tx_base_iova + (idx * PAGE_SIZE) as u64
    }
}

fn send_single_tx_complete(pool_idx: u16) {
    let mut complete = TxCompleteMsg {
        opcode: NET_CTRL_TX_COMPLETE,
        count: 1,
        pool_idx: [0; 20],
    };
    complete.pool_idx[0] = pool_idx;
    unsafe {
        DRIVER.send_tx_complete(&complete);
    }
}

static mut DRIVER: E1000Driver = E1000Driver::new();

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_service_endpoint();
    debug_write(b"e1000_driver: boot\n");
    unsafe {
        match DRIVER.init_hardware() {
            Ok(()) => {
                register_transport_endpoint();
                debug_write(b"e1000_driver: hardware ready\n");
            }
            Err(err) if err == syscall::ENOENT => {
                debug_write(b"e1000_driver: hardware absent\n");
            }
            Err(err) => debug_errno(b"e1000_driver: hardware unavailable errno ", err),
        }
    }

    let mut request = NetCtrlEnvelope::zeroed();
    loop {
        let rc = recv(&mut request);
        if rc == 9 {
            unsafe {
                DRIVER.poll_rx();
            }
            continue;
        }
        if rc > 0 {
            unsafe {
                match request.msg_type {
                    NET_CTRL_DRIVER_INIT => {
                        let init = core::ptr::read_unaligned(
                            request.payload.as_ptr() as *const DriverInitMsg
                        );
                        DRIVER.apply_driver_init(init);
                    }
                    NET_CTRL_RX_RELEASE => {
                        let msg = core::ptr::read_unaligned(
                            request.payload.as_ptr() as *const RxReleaseMsg
                        );
                        DRIVER.process_rx_releases(msg);
                    }
                    NET_CTRL_MAC_QUERY => DRIVER.send_mac_reply(),
                    NET_CTRL_TX_SUBMIT => {
                        let msg = core::ptr::read_unaligned(
                            request.payload.as_ptr() as *const TxSubmitMsg
                        );
                        DRIVER.process_tx_submit(msg);
                    }
                    _ => {}
                }
                DRIVER.poll_rx();
            }
        } else {
            unsafe {
                DRIVER.poll_rx();
            }
        }
    }
}

fn register_service_endpoint() {
    register_endpoint_name(b"e1000_driver");
}

fn register_transport_endpoint() {
    register_endpoint_name(b"e1000_net");
}

fn register_endpoint_name(name: &[u8]) {
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            ENDPOINT_ID,
        )
    };
}

fn recv(request: &mut NetCtrlEnvelope) -> i64 {
    unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            ENDPOINT_ID,
            request as *mut NetCtrlEnvelope as u64,
            core::mem::size_of::<NetCtrlEnvelope>() as u64,
            syscall::IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS,
        )
    }
}

fn send_ctrl(endpoint: u64, msg_type: u32, payload: &[u8]) {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) }.max(0) as u32;
    let mut msg = NetCtrlEnvelope {
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
            &msg as *const NetCtrlEnvelope as u64,
            core::mem::size_of::<NetCtrlEnvelope>() as u64,
            syscall::IPC_FLAG_INJECT_SRC_PID,
            0,
            0,
        )
    };
}

fn dma_alloc(size: usize) -> Result<DmaRegion, i64> {
    let mut virt = 0u64;
    let iova = unsafe {
        syscall::syscall5(
            syscall::SYS_DMA_ALLOC,
            size as u64,
            DMA_DIR_BIDIR,
            &mut virt as *mut u64 as u64,
            DMA_MAP_FLAGS_BYPASS_IOMMU,
            0,
        )
    };
    if iova < 0 {
        Err(iova)
    } else {
        Ok(DmaRegion {
            virt,
            iova: iova as u64,
        })
    }
}

#[inline]
unsafe fn read32(mmio: *mut u8, reg: usize) -> u32 {
    unsafe { core::ptr::read_volatile(mmio.add(reg) as *const u32) }
}

#[inline]
unsafe fn write32(mmio: *mut u8, reg: usize, value: u32) {
    unsafe { core::ptr::write_volatile(mmio.add(reg) as *mut u32, value) };
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
