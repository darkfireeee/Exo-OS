// kernel/src/memory/dma/engines/idxd.rs
//
// Intel Data Streaming Accelerator (DSA) / Intel Analytics Accelerator (IAX).
// Ref: Intel Architecture Specification for Intel DSA, Document 341204.
// Layer 0 — no_std kernel, MMIO access via raw pointers + ENQCMD instruction.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::PhysAddr;
use crate::memory::dma::stats::counters::{
    dma_stat_complete, dma_stat_error, dma_stat_submit, DMA_STATS,
};

// ─────────────────────────────────────────────────────────────────────────────
// IDXD MMIO REGISTERS
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
mod regs {
    /// Register GCAP — global capabilities (u64).
    pub const GCAP: usize = 0x00;
    /// Register GENSTATUS — global status (u32).
    pub const GENSTATUS: usize = 0x08;
    /// Register GENCTRL — global control (u32).
    pub const GENCTRL: usize = 0x0C;
    /// Register GENSTS — error status (u32 x 4).
    pub const GENSTS: usize = 0x20;
    /// Register INTCAUSE — interrupt cause (u32).
    pub const INTCAUSE: usize = 0x30;
    /// Work Queue register base (offset + 0x100 x wq_id).
    pub const WQ_BASE: usize = 0x100;
    pub const WQ_STRIDE: usize = 0x40;
    /// WQCFG register offset (Work Queue Config) in each WQ.
    pub const WQCFG: usize = 0x00;
    pub const WQCAP: usize = 0x04;
    pub const WQSTS: usize = 0x08;
    pub const WQDEPTH: usize = 0x0C;
}

/// GCAP bits.
#[allow(dead_code)]
mod gcap {
    pub const MAX_WQS_MASK: u64 = 0xF; // Bits 3:0
    pub const MAX_ENG_MASK: u64 = 0xF << 4; // Bits 7:4
    pub const NGROUPS_SHIFT: u32 = 8;
    pub const NGROUPS_MASK: u64 = 0xF << 8;
}

// ─────────────────────────────────────────────────────────────────────────────
// DSA DESCRIPTOR
// ─────────────────────────────────────────────────────────────────────────────

/// DSA operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DsaOpcode {
    Nop = 0x00,
    Batch = 0x01,
    Drain = 0x02,
    MemMove = 0x03,
    MemFill = 0x04,
    Compare = 0x05,
    CompPat = 0x06,
    CrcGen = 0x07,
    DifCheck = 0x08,
    DifInsert = 0x09,
    DifStrip = 0x0A,
    DifUpdate = 0x0B,
    CacheFlush = 0x0C,
}

/// DSA descriptor (64 bytes, cache-line aligned).
/// Format: Intel DSA Spec, Figure 3-1.
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct DsaDescriptor {
    /// Dword 0 : PASID (bits 19:0) + flags (bits 31:20).
    pub pasid_flags: u32,
    /// Dword 1 : opcode (bits 7:0) + flags (bits 31:8).
    pub opcode_flags: u32,
    /// Dword 2-3 : Completion Record Address (64 bits).
    pub completion_addr: u64,
    /// Dword 4-5 : Source Address 1 (64 bits).
    pub src1_addr: u64,
    /// Dword 6-7 : Destination Address (64 bits).
    pub dst_addr: u64,
    /// Dword 8 : Transfer Size.
    pub xfer_size: u32,
    /// Dword 9 : Descriptor Count (batch) or reserved.
    pub desc_count: u32,
    /// Dword 10-11 : Source Address 2 (for compare, etc.).
    pub src2_addr: u64,
    /// Dword 12-15 : reserved / operation-specific.
    pub aux: [u64; 2],
}

impl DsaDescriptor {
    /// Common flags.
    pub const FLAG_FENCE: u32 = 1 << 0; // Fence point
    pub const FLAG_BLOCK_ON_FAULT: u32 = 1 << 1; // Block on page fault
    pub const FLAG_COMP_ADDR: u32 = 1 << 2; // Valid completion address
    pub const FLAG_REQ_COMP_INT: u32 = 1 << 3; // Request interrupt
    pub const FLAG_CACHE_CTRL: u32 = 1 << 8; // Hint cache control

    /// Creates a MemMove descriptor (arbitrary DMA copy).
    pub fn new_memmove(src: PhysAddr, dst: PhysAddr, size: u32, cmp: PhysAddr) -> Self {
        DsaDescriptor {
            pasid_flags: 0,
            opcode_flags: (DsaOpcode::MemMove as u32) | Self::FLAG_COMP_ADDR,
            completion_addr: cmp.as_u64(),
            src1_addr: src.as_u64(),
            dst_addr: dst.as_u64(),
            xfer_size: size,
            desc_count: 0,
            src2_addr: 0,
            aux: [0; 2],
        }
    }

    /// Creates a MemFill descriptor.
    pub fn new_memfill(dst: PhysAddr, fill_val: u64, size: u32, cmp: PhysAddr) -> Self {
        DsaDescriptor {
            pasid_flags: 0,
            opcode_flags: (DsaOpcode::MemFill as u32) | Self::FLAG_COMP_ADDR,
            completion_addr: cmp.as_u64(),
            src1_addr: fill_val, // pattern for MemFill
            dst_addr: dst.as_u64(),
            xfer_size: size,
            desc_count: 0,
            src2_addr: 0,
            aux: [0; 2],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DSA COMPLETION RECORD
// ─────────────────────────────────────────────────────────────────────────────

/// DSA completion record (32 bytes).
#[repr(C, align(32))]
#[derive(Debug, Clone, Copy)]
pub struct DsaCompletionRecord {
    /// Status (0 = in progress, 1 = success, others = errors).
    pub status: u8,
    /// Diagnostic alerts.
    pub result: u8,
    /// Reserved.
    pub _rsvd: u16,
    /// Transferred bytes (for partial operations).
    pub bytes_done: u32,
    /// Fault address (if page fault).
    pub fault_addr: u64,
    /// Reserved.
    pub _aux: [u64; 2],
}

impl DsaCompletionRecord {
    const fn new() -> Self {
        DsaCompletionRecord {
            status: 0,
            result: 0,
            _rsvd: 0,
            bytes_done: 0,
            fault_addr: 0,
            _aux: [0; 2],
        }
    }

    pub fn is_done(&self) -> bool {
        self.status != 0
    }
    pub fn is_success(&self) -> bool {
        self.status == 1
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WORK QUEUE
// ─────────────────────────────────────────────────────────────────────────────

pub const IDXD_WQ_DEPTH: usize = 128;

struct IdxdWqState {
    descs: [DsaDescriptor; IDXD_WQ_DEPTH],
    records: [DsaCompletionRecord; IDXD_WQ_DEPTH],
    head: u8,
    count: u8,
}

impl IdxdWqState {
    const fn new() -> Self {
        IdxdWqState {
            descs: [DsaDescriptor {
                pasid_flags: 0,
                opcode_flags: 0,
                completion_addr: 0,
                src1_addr: 0,
                dst_addr: 0,
                xfer_size: 0,
                desc_count: 0,
                src2_addr: 0,
                aux: [0; 2],
            }; IDXD_WQ_DEPTH],
            records: [DsaCompletionRecord::new(); IDXD_WQ_DEPTH],
            head: 0,
            count: 0,
        }
    }

    fn is_full(&self) -> bool {
        self.count as usize >= IDXD_WQ_DEPTH
    }
    #[allow(dead_code)]
    fn available(&self) -> usize {
        IDXD_WQ_DEPTH - self.count as usize
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IDXD ENGINE
// ─────────────────────────────────────────────────────────────────────────────

/// Intel DSA engine.
pub struct IdxdEngine {
    mmio_base: AtomicU64,
    /// Work Queue 0 submission portal address (MMIO movdir64b).
    wq_portal: AtomicU64,
    present: AtomicBool,
    engine_id: AtomicU32,
    wq: Mutex<IdxdWqState>,
}

impl IdxdEngine {
    const fn new() -> Self {
        IdxdEngine {
            mmio_base: AtomicU64::new(0),
            wq_portal: AtomicU64::new(0),
            present: AtomicBool::new(false),
            engine_id: AtomicU32::new(0),
            wq: Mutex::new(IdxdWqState::new()),
        }
    }

    unsafe fn read64(&self, offset: usize) -> u64 {
        let base = self.mmio_base.load(Ordering::Relaxed) as *const u64;
        base.add(offset / 8).read_volatile()
    }

    unsafe fn write32(&self, offset: usize, val: u32) {
        let base = self.mmio_base.load(Ordering::Relaxed) as *mut u32;
        base.add(offset / 4).write_volatile(val);
    }

    /// Initializes the DSA engine.
    ///
    /// `mmio_base`: PCI MMIO BAR base.
    /// `wq_portal`: WQ0 submission portal address (BAR2).
    ///
    /// # Safety
    /// CPL 0, valid MMIOs, CPU supports ENQCMD.
    pub unsafe fn init(&self, mmio_base: u64, wq_portal: u64) -> bool {
        self.mmio_base.store(mmio_base, Ordering::Release);
        self.wq_portal.store(wq_portal, Ordering::Release);

        // Check GCAP: number of WQs and engines.
        let gcap = self.read64(regs::GCAP);
        let max_wqs = (gcap & gcap::MAX_WQS_MASK) as u32;
        let max_engs = ((gcap & gcap::MAX_ENG_MASK) >> 4) as u32;
        if max_wqs == 0 || max_engs == 0 {
            return false;
        }

        // Enable the device (bit 0 of GENCTRL).
        self.write32(regs::GENCTRL, 0x1);

        // Wait for GENSTATUS.DEVSTATE == Enabled (bits 1:0 = 0b01).
        let genstatus_base = self.mmio_base.load(Ordering::Relaxed) as *const u32;
        for _ in 0..10_000 {
            let s = genstatus_base.add(regs::GENSTATUS / 4).read_volatile();
            if s & 0x3 == 1 {
                break;
            }
        }

        let engine_id = DMA_STATS.register_engine();
        self.engine_id.store(engine_id as u32, Ordering::Relaxed);
        self.present.store(true, Ordering::Release);
        true
    }

    /// Submits a DSA descriptor via ENQCMD to the WQ0 portal.
    ///
    /// # Safety
    /// Valid MMIO portal, CPU with ENQCMD, valid descriptor.
    pub unsafe fn submit(&self, desc: DsaDescriptor) -> bool {
        if !self.present.load(Ordering::Acquire) {
            return false;
        }
        let mut wq = self.wq.lock();
        if wq.is_full() {
            return false;
        }

        let slot = wq.head as usize;
        wq.descs[slot] = desc;
        wq.count += 1;
        wq.head = ((wq.head as usize + 1) % IDXD_WQ_DEPTH) as u8;

        // Submit via ENQCMD (movdir64b or enqcmd instruction depending on mode).
        // ENQCMD atomically sends 64 bytes to the WQ portal.
        let portal = self.wq_portal.load(Ordering::Relaxed) as *mut u8;
        let desc_ptr = &wq.descs[slot] as *const DsaDescriptor as *const u8;

        // Copy 64 bytes to the portal (MMIO movdir64b).
        // In production: use `enqcmd` or `movdir64b` via asm!.
        // Here: write_volatile of 8 quadwords.
        let portal64 = portal as *mut u64;
        let desc64 = desc_ptr as *const u64;
        for i in 0..8 {
            portal64.add(i).write_volatile(desc64.add(i).read());
        }
        // Fence: ensure the write is visible to HW.
        core::sync::atomic::fence(Ordering::SeqCst);

        drop(wq);
        let eid = self.engine_id.load(Ordering::Relaxed) as usize;
        dma_stat_submit(eid);
        true
    }

    /// Polls completions via completion records.
    ///
    /// # Safety
    /// Called in a non-preemptible context.
    pub unsafe fn poll(&self) -> usize {
        if !self.present.load(Ordering::Acquire) {
            return 0;
        }
        let mut wq = self.wq.lock();
        let mut completed = 0usize;
        let eid = self.engine_id.load(Ordering::Relaxed) as usize;

        for i in 0..wq.count as usize {
            let slot = (wq.head as usize + IDXD_WQ_DEPTH - wq.count as usize + i) % IDXD_WQ_DEPTH;
            // Completion records are updated by HW in memory.
            // Read the record status for this slot.
            if wq.records[slot].is_done() {
                if wq.records[slot].is_success() {
                    dma_stat_complete(eid, wq.records[slot].bytes_done as u64, true, 0);
                } else {
                    dma_stat_error(eid);
                }
                // Reset the record.
                wq.records[slot].status = 0;
                completed += 1;
                wq.count -= 1;
            }
        }
        completed
    }

    /// Submits a DSA memory copy.
    ///
    /// # Safety
    /// Valid physical addresses.
    pub unsafe fn submit_memmove(
        &self,
        src: PhysAddr,
        dst: PhysAddr,
        size: u32,
        cmp: PhysAddr,
    ) -> bool {
        self.submit(DsaDescriptor::new_memmove(src, dst, size, cmp))
    }
}

// SAFETY: IdxdEngine uses Mutex + atomics.
unsafe impl Sync for IdxdEngine {}

/// Global instance of the IDXD/DSA engine.
pub static IDXD_ENGINE: IdxdEngine = IdxdEngine::new();

/// Initializes the IDXD engine.
///
/// # Safety
/// Valid MMIOs.
pub unsafe fn idxd_init(mmio_base: u64, wq_portal: u64) -> bool {
    IDXD_ENGINE.init(mmio_base, wq_portal)
}

/// Submits a DSA operation.
///
/// # Safety
/// Valid physical addresses.
pub unsafe fn idxd_submit(desc: DsaDescriptor) -> bool {
    IDXD_ENGINE.submit(desc)
}

/// Polls DSA completions.
///
/// # Safety
/// Non-preemptible context.
pub unsafe fn idxd_poll() -> usize {
    IDXD_ENGINE.poll()
}
