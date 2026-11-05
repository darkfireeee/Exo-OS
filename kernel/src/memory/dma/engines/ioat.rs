// kernel/src/memory/dma/engines/ioat.rs
//
// Intel I/OAT DMA Engine (Crystal Beach / Xeon series).
// Ref: Intel I/OAT DMA Engine User Guide, Doc #322931.
// Layer 0 — no_std kernel, MMIO access via raw pointers.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::PhysAddr;
use crate::memory::dma::stats::counters::{
    dma_stat_complete, dma_stat_error, dma_stat_submit, DMA_STATS,
};

// ─────────────────────────────────────────────────────────────────────────────
// IOAT MMIO REGISTERS (offset from MMIO base)
// ─────────────────────────────────────────────────────────────────────────────

/// Register offsets for I/OAT channel 0 (CB3+).
#[allow(dead_code)]
mod regs {
    pub const CHANCMD: usize = 0x04; // Channel Command (u8)
    pub const XFERCAP: usize = 0x06; // Transfer Capability (u8)
    pub const GENCTRL: usize = 0x08; // General Control (u8)
    pub const INTRCTRL: usize = 0x0C; // Interrupt Control (u16)
    pub const ATTNSTATUS: usize = 0x10; // Attention Status (u32)
    pub const CHANCTRL: usize = 0x80; // Channel Control (u16) — per channel
    pub const DMACOUNT: usize = 0x84; // DMA Transfer Count (u16)
    pub const CHANSTS_LO: usize = 0x88; // Channel Status Low (u32)
    pub const CHANSTS_HI: usize = 0x8C; // Channel Status High (u32)
    pub const CHAINADDR_LO: usize = 0x90; // Descriptor Chain Address Low (u32)
    pub const CHAINADDR_HI: usize = 0x94; // Descriptor Chain Address High (u32)
    pub const CHANCMP_LO: usize = 0x98; // Completion Buffer Address Low (u32)
    pub const CHANCMP_HI: usize = 0x9C; // Completion Buffer Address High (u32)
    pub const DCACTRL: usize = 0xB0; // DCA Control (u32)
}

/// Channel Status Register bits.
#[allow(dead_code)]
mod chansts {
    pub const ACTIVE: u64 = 0x0; // Active channel (bits 4:0)
    pub const DONE: u64 = 0x1; // Last descriptor completed
    pub const SUSPENDED: u64 = 0x2; // Channel suspended
    pub const HALTED: u64 = 0x3; // Fatal error
    pub const ARMED: u64 = 0x4; // Awaiting a descriptor
    pub const STATUS_MASK: u64 = 0x7;
}

/// Channel Control Register bits.
#[allow(dead_code)]
mod chanctrl {
    pub const INT_DISABLE: u16 = 1 << 0; // Disable interrupts
    pub const ERR_INT: u16 = 1 << 2; // Error -> interrupt
    pub const ANY_ERR_ABORT: u16 = 1 << 3; // Abort on any error
}

// ─────────────────────────────────────────────────────────────────────────────
// I/OAT DESCRIPTOR (CB3)
// ─────────────────────────────────────────────────────────────────────────────

/// I/OAT DMA Engine descriptor version CB3 (DMA Copy).
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct IoatDescriptor {
    /// Transfer length in bytes (bits 0:23), flags (bits 24:31).
    pub size_flags: u32,
    /// Destination physical address (64 bits).
    pub dst_lo: u32,
    pub dst_hi: u32,
    /// Source physical address (64 bits).
    pub src_lo: u32,
    pub src_hi: u32,
    /// Next descriptor address (NULL = end of list).
    pub next_lo: u32,
    pub next_hi: u32,
    /// Completion record address (64 bits).
    pub cmp_lo: u32,
    pub cmp_hi: u32,
    /// Additional control flags (CB3: reserved).
    pub ctrl_result: u32,
    pub reserved: u32,
    pub user_data: u64,
}

impl IoatDescriptor {
    pub const FLAG_INT_EN: u32 = 1 << 24; // Interrupt on completion
    pub const FLAG_DST_SNOOP: u32 = 1 << 25; // Destination snooping
    pub const FLAG_SRC_SNOOP: u32 = 1 << 26; // Source snooping
    pub const FLAG_CMP_ADDR: u32 = 1 << 27; // Write completion address
    pub const FLAG_FENCE: u32 = 1 << 28; // Memory fence after copy
    pub const FLAG_NULL: u32 = 1 << 31; // Null descriptor (NOP)

    /// Creates a DMA copy descriptor.
    pub fn new_copy(src: PhysAddr, dst: PhysAddr, size: u32) -> Self {
        IoatDescriptor {
            size_flags: size & 0x00FF_FFFF, // bits 23:0 = size
            dst_lo: dst.as_u64() as u32,
            dst_hi: (dst.as_u64() >> 32) as u32,
            src_lo: src.as_u64() as u32,
            src_hi: (src.as_u64() >> 32) as u32,
            next_lo: 0,
            next_hi: 0,
            cmp_lo: 0,
            cmp_hi: 0,
            ctrl_result: 0,
            reserved: 0,
            user_data: 0,
        }
    }

    /// Chains this descriptor to the next.
    pub fn set_next(&mut self, next_phys: PhysAddr) {
        self.next_lo = next_phys.as_u64() as u32;
        self.next_hi = (next_phys.as_u64() >> 32) as u32;
    }

    /// Sets the completion record address.
    pub fn set_completion_addr(&mut self, cmp_phys: PhysAddr) {
        self.cmp_lo = cmp_phys.as_u64() as u32;
        self.cmp_hi = (cmp_phys.as_u64() >> 32) as u32;
        self.size_flags |= Self::FLAG_CMP_ADDR;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IOAT DESCRIPTOR RING
// ─────────────────────────────────────────────────────────────────────────────

/// Size of the I/OAT descriptor ring (must be a power of 2).
pub const IOAT_RING_SIZE: usize = 256;

/// I/OAT descriptor ring.
struct IoatRing {
    descs: [IoatDescriptor; IOAT_RING_SIZE],
    /// Completion records (one per slot, 64-bit aligned).
    #[allow(dead_code)]
    cmp: [u64; IOAT_RING_SIZE],
    head: u16,  // next slot to write
    tail: u16,  // next slot to complete
    count: u16, // in-flight descriptors
}

impl IoatRing {
    const fn new() -> Self {
        IoatRing {
            descs: [IoatDescriptor {
                size_flags: 0,
                dst_lo: 0,
                dst_hi: 0,
                src_lo: 0,
                src_hi: 0,
                next_lo: 0,
                next_hi: 0,
                cmp_lo: 0,
                cmp_hi: 0,
                ctrl_result: 0,
                reserved: 0,
                user_data: 0,
            }; IOAT_RING_SIZE],
            cmp: [0u64; IOAT_RING_SIZE],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    #[allow(dead_code)]
    fn available(&self) -> usize {
        IOAT_RING_SIZE - self.count as usize - 1
    }

    fn is_full(&self) -> bool {
        self.count as usize >= IOAT_RING_SIZE - 1
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IOAT ENGINE
// ─────────────────────────────────────────────────────────────────────────────

/// I/OAT DMA engine.
pub struct IoatEngine {
    /// MMIO base of channel 0.
    mmio_base: AtomicU64,
    /// Engine present and initialized.
    present: AtomicBool,
    /// Stats identifier.
    engine_id: AtomicU32,
    /// Descriptor ring.
    ring: Mutex<IoatRing>,
}

impl IoatEngine {
    const fn new() -> Self {
        IoatEngine {
            mmio_base: AtomicU64::new(0),
            present: AtomicBool::new(false),
            engine_id: AtomicU32::new(0),
            ring: Mutex::new(IoatRing::new()),
        }
    }

    /// Reads an 8-bit register.
    ///
    /// # Safety: `mmio_base` must be a valid and mapped MMIO address.
    unsafe fn read8(&self, offset: usize) -> u8 {
        let base = self.mmio_base.load(Ordering::Relaxed) as *const u8;
        base.add(offset).read_volatile()
    }

    /// Lit un registre 16 bits.
    #[allow(dead_code)]
    unsafe fn read16(&self, offset: usize) -> u16 {
        let base = self.mmio_base.load(Ordering::Relaxed) as *const u16;
        base.add(offset / 2).read_volatile()
    }

    /// Reads a 32-bit register.
    unsafe fn read32(&self, offset: usize) -> u32 {
        let base = self.mmio_base.load(Ordering::Relaxed) as *const u32;
        base.add(offset / 4).read_volatile()
    }

    /// Writes a 16-bit register.
    unsafe fn write16(&self, offset: usize, val: u16) {
        let base = self.mmio_base.load(Ordering::Relaxed) as *mut u16;
        base.add(offset / 2).write_volatile(val);
    }

    /// Writes a 32-bit register.
    #[allow(dead_code)]
    unsafe fn write32(&self, offset: usize, val: u32) {
        let base = self.mmio_base.load(Ordering::Relaxed) as *mut u32;
        base.add(offset / 4).write_volatile(val);
    }

    /// Reads the channel status.
    unsafe fn chan_status(&self) -> u64 {
        let lo = self.read32(regs::CHANSTS_LO) as u64;
        let hi = self.read32(regs::CHANSTS_HI) as u64;
        lo | (hi << 32)
    }

    /// Initializes the I/OAT channel.
    ///
    /// # Safety
    /// CPL 0, `mmio_base` valid and mapped in WB.
    pub unsafe fn init(&self, mmio_base: u64) -> bool {
        self.mmio_base.store(mmio_base, Ordering::Release);

        // 1. Read transfer capability (XFERCAP).
        let xfercap = self.read8(regs::XFERCAP);
        if xfercap == 0 {
            return false;
        } // Controller absent or invalid

        // 2. Disable interrupts, enable stop on error.
        let ctrl = chanctrl::INT_DISABLE | chanctrl::ANY_ERR_ABORT;
        self.write16(regs::CHANCTRL, ctrl);

        // 3. Reset channel (RESET = 0x20 in CHANCMD).
        let cmd_offs = regs::CHANCMD;
        let base = mmio_base as *mut u8;
        base.add(cmd_offs).write_volatile(0x20); // CHANCMD_RESET

        // 4. Wait for reset completion (max 1000 reads).
        for _ in 0..1000 {
            let status = self.chan_status() & chansts::STATUS_MASK;
            if status != chansts::HALTED {
                break;
            }
        }

        let engine_id = DMA_STATS.register_engine();
        self.engine_id.store(engine_id as u32, Ordering::Relaxed);
        self.present.store(true, Ordering::Release);
        true
    }

    /// Submits a DMA copy.
    ///
    /// `src` / `dst`: physical addresses. `size` in bytes (<= 4 MiB per desc.).
    /// Returns the slot index or None if ring is full.
    ///
    /// # Safety
    /// Valid physical addresses, mapped in physmap.
    pub unsafe fn submit_copy(&self, src: PhysAddr, dst: PhysAddr, size: u32) -> Option<u16> {
        if !self.present.load(Ordering::Acquire) {
            return None;
        }
        let mut ring = self.ring.lock();
        if ring.is_full() {
            return None;
        }

        let slot = ring.head;
        let next_slot = (slot + 1) as usize % IOAT_RING_SIZE;

        // Build descriptor.
        ring.descs[slot as usize] = IoatDescriptor::new_copy(src, dst, size);

        // Physical address of the completion record for this slot.
        // In production, records are in a dedicated physical page allocated at init.
        // Here: use address of cmp[] field via physmap — simplified for init.
        ring.count += 1;
        ring.head = next_slot as u16;

        // Bump DMACOUNT to trigger the engine (16-bit register of desc count).
        self.write16(regs::DMACOUNT, ring.count);

        let eid = self.engine_id.load(Ordering::Relaxed) as usize;
        dma_stat_submit(eid);

        Some(slot)
    }

    /// Polls completions — returns the number of completed transfers.
    ///
    /// # Safety
    /// Called from a non-preemptible context (IRQ off or spinlock).
    pub unsafe fn poll(&self) -> usize {
        if !self.present.load(Ordering::Acquire) {
            return 0;
        }
        let status = self.chan_status();
        let state = status & chansts::STATUS_MASK;
        let eid = self.engine_id.load(Ordering::Relaxed) as usize;

        if state == chansts::DONE {
            let mut ring = self.ring.lock();
            let completed = ring.count as usize;
            ring.count = 0;
            ring.tail = ring.head;
            drop(ring);
            dma_stat_complete(eid, 0, true, 0);
            return completed;
        }

        if state == chansts::HALTED {
            // Fatal error — reset the channel.
            let base = self.mmio_base.load(Ordering::Relaxed) as *mut u8;
            base.add(regs::CHANCMD).write_volatile(0x20); // RESET
            let mut ring = self.ring.lock();
            let failed = ring.count as usize;
            ring.count = 0;
            ring.tail = ring.head;
            drop(ring);
            dma_stat_error(eid);
            let _ = failed;
        }
        0
    }
}

// SAFETY: IoatEngine uses an internal Mutex and atomic accesses.
unsafe impl Sync for IoatEngine {}

/// Global instance of the I/OAT engine.
pub static IOAT_ENGINE: IoatEngine = IoatEngine::new();

/// Initializes the I/OAT engine.
///
/// # Safety
/// `mmio_base` = base of the PCI MMIO BAR of the I/OAT controller.
pub unsafe fn ioat_init(mmio_base: u64) -> bool {
    IOAT_ENGINE.init(mmio_base)
}

/// Submits a DMA copy via I/OAT.
///
/// # Safety
/// Valid physical addresses.
pub unsafe fn ioat_submit(src: PhysAddr, dst: PhysAddr, size: u32) -> Option<u16> {
    IOAT_ENGINE.submit_copy(src, dst, size)
}

/// Polls I/OAT completions.
///
/// # Safety
/// Non-preemptible context.
pub unsafe fn ioat_poll() -> usize {
    IOAT_ENGINE.poll()
}
