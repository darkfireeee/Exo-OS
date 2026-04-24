// kernel/src/memory/dma/engines/nvme_dma.rs
//
// Moteur DMA NVMe PCIe (NVM Express 1.4).
// Ref : NVM Express Base Specification, Revision 1.4c.
// Couche 0 — no_std, accès MMIO BAR0 via raw pointers.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::PhysAddr;
use crate::memory::dma::stats::counters::{
    dma_stat_complete, dma_stat_error, dma_stat_submit, DMA_STATS,
};

// ─────────────────────────────────────────────────────────────────────────────
// REGISTRES CONTROLLER MMIO
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
mod regs {
    /// Controller Capabilities (64 bits).
    pub const CAP: usize = 0x00;
    /// Version (32 bits).
    pub const VS: usize = 0x08;
    /// Controller Configuration (32 bits).
    pub const CC: usize = 0x14;
    /// Controller Status (32 bits).
    pub const CSTS: usize = 0x1C;
    /// Admin Queue Attributes (32 bits).
    pub const AQA: usize = 0x24;
    /// Admin Submission Queue Base Address (64 bits).
    pub const ASQ: usize = 0x28;
    /// Admin Completion Queue Base Address (64 bits).
    pub const ACQ: usize = 0x30;
    /// Submission Queue 0 Tail Doorbell (stride-dependent).
    pub const SQ0TDB: usize = 0x1000;
    /// Completion Queue 0 Head Doorbell (stride).
    pub const CQ0HDB: usize = 0x1004;
    /// I/O Submission Queue 1 Tail Doorbell.
    pub const SQ1TDB: usize = 0x1008;
    /// I/O Completion Queue 1 Head Doorbell.
    pub const CQ1HDB: usize = 0x100C;
}

mod cc {
    pub const EN: u32 = 1 << 0;
    pub const IO_SQS_SHIFT: u32 = 16; // I/O SQ entry size (log2)
    pub const IO_CQS_SHIFT: u32 = 20; // I/O CQ entry size (log2)
    pub const IO_SQS_64: u32 = 6; // 64 bytes
    pub const IO_CQS_16: u32 = 4; // 16 bytes
    pub const CSS_NVM: u32 = 0 << 4; // NVM command set
    pub const MPS_4K: u32 = 0 << 7; // 4096 bytes pages
}

mod csts {
    pub const RDY: u32 = 1 << 0;
    pub const CFS: u32 = 1 << 1; // Controller Fatal Status
}

// ─────────────────────────────────────────────────────────────────────────────
// OPCODE NVM
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NvmeAdminOpc {
    CreateIOCQ = 0x05,
    CreateIOSQ = 0x01,
    Identify = 0x06,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NvmeIoOpc {
    Flush = 0x00,
    Write = 0x01,
    Read = 0x02,
}

// ─────────────────────────────────────────────────────────────────────────────
// SUBMISSION QUEUE ENTRY (SQE) — 64 bytes
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, align(64))]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmeSqe {
    /// DW0 : CDW0 — opcode (7:0), fuse (9:8), CID (31:16).
    pub cdw0: u32,
    /// DW1 : NSID.
    pub nsid: u32,
    /// DW2-DW3 : CDW2-CDW3 (réservés pour NVM).
    pub cdw2: u32,
    pub cdw3: u32,
    /// DW4-DW5 : Metadata Pointer.
    pub mptr_lo: u32,
    pub mptr_hi: u32,
    /// DW6-DW9 : Data Pointer (PRP1, PRP2 ou SGL).
    pub prp1_lo: u32,
    pub prp1_hi: u32,
    pub prp2_lo: u32,
    pub prp2_hi: u32,
    /// DW10-DW15 : opération-dépendants.
    pub cdw10: u32,
    pub cdw11: u32,
    pub cdw12: u32,
    pub cdw13: u32,
    pub cdw14: u32,
    pub cdw15: u32,
}

impl NvmeSqe {
    pub fn new_read(cid: u16, nsid: u32, slba: u64, nr_minus1: u16, prp1: PhysAddr) -> Self {
        NvmeSqe {
            cdw0: (NvmeIoOpc::Read as u32) | ((cid as u32) << 16),
            nsid,
            cdw2: 0,
            cdw3: 0,
            mptr_lo: 0,
            mptr_hi: 0,
            prp1_lo: (prp1.as_u64() & 0xFFFF_FFFF) as u32,
            prp1_hi: (prp1.as_u64() >> 32) as u32,
            prp2_lo: 0,
            prp2_hi: 0,
            cdw10: (slba & 0xFFFF_FFFF) as u32,
            cdw11: (slba >> 32) as u32,
            cdw12: nr_minus1 as u32, // NLB (0-based)
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }

    pub fn new_write(cid: u16, nsid: u32, slba: u64, nr_minus1: u16, prp1: PhysAddr) -> Self {
        let mut s = Self::new_read(cid, nsid, slba, nr_minus1, prp1);
        s.cdw0 = (NvmeIoOpc::Write as u32) | ((cid as u32) << 16);
        s
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// COMPLETION QUEUE ENTRY (CQE) — 16 bytes
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmeCqe {
    /// DW0 : Command Specific.
    pub dw0: u32,
    /// DW1 : réservé.
    pub dw1: u32,
    /// DW2 : SQ Head Pointer (15:0) + SQ ID (31:16).
    pub dw2: u32,
    /// DW3 : CID (31:16) + Phase Tag (0) + Status (15:1).
    pub dw3: u32,
}

impl NvmeCqe {
    pub fn phase(&self) -> bool {
        self.dw3 & 1 != 0
    }
    pub fn status(&self) -> u16 {
        ((self.dw3 >> 1) & 0x7FFF) as u16
    }
    pub fn cid(&self) -> u16 {
        (self.dw3 >> 16) as u16
    }
    pub fn is_success(&self) -> bool {
        self.status() == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// QUEUES INTERNE
// ─────────────────────────────────────────────────────────────────────────────

const NVME_QUEUE_DEPTH: usize = 64;

struct NvmeSQ {
    entries: [NvmeSqe; NVME_QUEUE_DEPTH],
    tail: u16,
    #[allow(dead_code)]
    head: u16,
    count: u16,
}

impl NvmeSQ {
    #[allow(dead_code)]
    fn new() -> Self {
        NvmeSQ {
            entries: [NvmeSqe::default(); NVME_QUEUE_DEPTH],
            tail: 0,
            head: 0,
            count: 0,
        }
    }
    fn is_full(&self) -> bool {
        self.count as usize >= NVME_QUEUE_DEPTH
    }
    fn submit(&mut self, sqe: NvmeSqe) -> bool {
        if self.is_full() {
            return false;
        }
        self.entries[self.tail as usize] = sqe;
        self.tail = (self.tail + 1) % NVME_QUEUE_DEPTH as u16;
        self.count += 1;
        true
    }
}

struct NvmeCQ {
    entries: [NvmeCqe; NVME_QUEUE_DEPTH],
    head: u16,
    phase: bool,
}

impl NvmeCQ {
    fn new() -> Self {
        NvmeCQ {
            entries: [NvmeCqe::default(); NVME_QUEUE_DEPTH],
            head: 0,
            phase: true,
        }
    }

    /// Retourne la prochaine entrée complétée, si disponible.
    fn pop(&mut self) -> Option<NvmeCqe> {
        let cqe = self.entries[self.head as usize];
        if cqe.phase() != self.phase {
            return None;
        }
        self.head = (self.head + 1) % NVME_QUEUE_DEPTH as u16;
        if self.head == 0 {
            self.phase = !self.phase;
        }
        Some(cqe)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MOTEUR NVME DMA
// ─────────────────────────────────────────────────────────────────────────────

struct NvmeState {
    sq: NvmeSQ,
    cq: NvmeCQ,
    cid_counter: u16,
    bytes_inflight: [u32; NVME_QUEUE_DEPTH],
    is_write: [bool; NVME_QUEUE_DEPTH],
}

impl NvmeState {
    fn new() -> Self {
        NvmeState {
            sq: NvmeSQ::new(),
            cq: NvmeCQ::new(),
            cid_counter: 1,
            bytes_inflight: [0; NVME_QUEUE_DEPTH],
            is_write: [false; NVME_QUEUE_DEPTH],
        }
    }

    fn next_cid(&mut self) -> u16 {
        let c = self.cid_counter;
        self.cid_counter = self.cid_counter.wrapping_add(1).max(1);
        c
    }
}

pub struct NvmeDmaEngine {
    bar0: AtomicU64,
    present: AtomicBool,
    engine_id: AtomicU32,
    nsid: AtomicU32,
    state: Mutex<NvmeState>,
}

impl NvmeDmaEngine {
    const fn new_uninit() -> Self {
        NvmeDmaEngine {
            bar0: AtomicU64::new(0),
            present: AtomicBool::new(false),
            engine_id: AtomicU32::new(0),
            nsid: AtomicU32::new(1),
            state: Mutex::new(unsafe { core::mem::zeroed() }),
        }
    }

    unsafe fn read32(&self, off: usize) -> u32 {
        let p = (self.bar0.load(Ordering::Relaxed) + off as u64) as *const u32;
        p.read_volatile()
    }

    unsafe fn write32(&self, off: usize, v: u32) {
        let p = (self.bar0.load(Ordering::Relaxed) + off as u64) as *mut u32;
        p.write_volatile(v);
    }

    unsafe fn write64(&self, off: usize, v: u64) {
        let p = (self.bar0.load(Ordering::Relaxed) + off as u64) as *mut u64;
        p.write_volatile(v);
    }

    /// Initialise le contrôleur NVMe.
    ///
    /// # Safety : BAR0 MMIO valide, SQ/CQ en mémoire physique identité-mappée.
    pub unsafe fn init(&self, bar0: u64, nsid: u32) -> bool {
        self.bar0.store(bar0, Ordering::Release);
        self.nsid.store(nsid, Ordering::Relaxed);

        // Réinitialiser l'état des queues (phase CQ = true obligatoirement).
        {
            let mut st = self.state.lock();
            *st = NvmeState::new();
        }

        // Désactiver le contrôleur (CC.EN = 0) et attendre CSTS.RDY = 0.
        let mut cc = self.read32(regs::CC);
        cc &= !cc::EN;
        self.write32(regs::CC, cc);
        let mut tries = 100_000u32;
        while tries > 0 {
            if self.read32(regs::CSTS) & csts::RDY == 0 {
                break;
            }
            tries -= 1;
        }
        if tries == 0 {
            return false;
        }

        // Configurer les queues via les adresses statiques en state.
        let st = self.state.lock();
        let sq_phys = &st.sq.entries as *const _ as u64;
        let cq_phys = &st.cq.entries as *const _ as u64;
        drop(st);

        // Admin queues → on utilise les I/O queues (pas d'admin ici).
        self.write64(regs::ASQ, sq_phys);
        self.write64(regs::ACQ, cq_phys);
        self.write32(
            regs::AQA,
            ((NVME_QUEUE_DEPTH as u32 - 1) << 16) | (NVME_QUEUE_DEPTH as u32 - 1),
        );

        // Activer CC.EN avec tailles de queue.
        let cc_cfg = cc::EN
            | (cc::IO_SQS_64 << cc::IO_SQS_SHIFT)
            | (cc::IO_CQS_16 << cc::IO_CQS_SHIFT)
            | cc::CSS_NVM
            | cc::MPS_4K;
        self.write32(regs::CC, cc_cfg);

        // Attendre CSTS.RDY = 1.
        let mut tries = 100_000u32;
        while tries > 0 {
            let csts = self.read32(regs::CSTS);
            if csts & csts::CFS != 0 {
                return false;
            }
            if csts & csts::RDY != 0 {
                break;
            }
            tries -= 1;
        }
        if tries == 0 {
            return false;
        }

        let eid = DMA_STATS.register_engine();
        self.engine_id.store(eid as u32, Ordering::Relaxed);
        self.present.store(true, Ordering::Release);
        true
    }

    /// Soumet une lecture NVMe.
    ///
    /// # Safety : `prp1` est une adresse physique 4 KiB-alignée, taille <= 4096.
    pub unsafe fn submit_read(&self, slba: u64, nlb: u16, prp1: PhysAddr) -> bool {
        self.submit_io(slba, nlb, prp1, false)
    }

    /// Soumet une écriture NVMe.
    ///
    /// # Safety : idem submit_read.
    pub unsafe fn submit_write(&self, slba: u64, nlb: u16, prp1: PhysAddr) -> bool {
        self.submit_io(slba, nlb, prp1, true)
    }

    unsafe fn submit_io(&self, slba: u64, nlb: u16, prp1: PhysAddr, write: bool) -> bool {
        if !self.present.load(Ordering::Acquire) {
            return false;
        }
        let mut st = self.state.lock();
        if st.sq.is_full() {
            return false;
        }

        let cid = st.next_cid();
        let nsid = self.nsid.load(Ordering::Relaxed);
        let sqe = if write {
            NvmeSqe::new_write(cid, nsid, slba, nlb, prp1)
        } else {
            NvmeSqe::new_read(cid, nsid, slba, nlb, prp1)
        };

        let slot = cid as usize % NVME_QUEUE_DEPTH;
        st.bytes_inflight[slot] = (nlb as u32 + 1) * 512;
        st.is_write[slot] = write;

        if !st.sq.submit(sqe) {
            return false;
        }

        // Ring Submission Doorbell.
        let tail = st.sq.tail;
        drop(st);
        self.write32(regs::SQ1TDB, tail as u32);

        let eid = self.engine_id.load(Ordering::Relaxed) as usize;
        dma_stat_submit(eid);
        true
    }

    /// Sonde la Completion Queue pour les entrées terminées.
    ///
    /// # Safety : contexte non-préemptible.
    pub unsafe fn poll_cq(&self) -> usize {
        if !self.present.load(Ordering::Acquire) {
            return 0;
        }
        let mut st = self.state.lock();
        let eid = self.engine_id.load(Ordering::Relaxed) as usize;
        let mut count = 0;

        while let Some(cqe) = st.cq.pop() {
            let slot = cqe.cid() as usize % NVME_QUEUE_DEPTH;
            if cqe.is_success() {
                let bytes = st.bytes_inflight[slot] as u64;
                let write = st.is_write[slot];
                dma_stat_complete(eid, bytes, write, 0);
            } else {
                dma_stat_error(eid);
            }
            count += 1;
        }

        // Ring CQ Head Doorbell si des entrées ont été consommées.
        if count > 0 {
            let head = st.cq.head;
            drop(st);
            self.write32(regs::CQ1HDB, head as u32);
        }
        count
    }
}

// SAFETY: NvmeDmaEngine utilise Mutex + atomics.
unsafe impl Sync for NvmeDmaEngine {}

/// Instance globale du moteur NVMe DMA.
pub static NVME_DMA: NvmeDmaEngine = NvmeDmaEngine::new_uninit();

/// Initialise le moteur NVMe DMA.
///
/// # Safety : BAR0 valide.
pub unsafe fn nvme_dma_init(bar0: u64, nsid: u32) -> bool {
    NVME_DMA.init(bar0, nsid)
}

/// Lit des secteurs NVMe.
///
/// # Safety : `prp1` valide.
pub unsafe fn nvme_read(slba: u64, nlb: u16, prp1: PhysAddr) -> bool {
    NVME_DMA.submit_read(slba, nlb, prp1)
}

/// Écrit des secteurs NVMe.
///
/// # Safety : `prp1` valide.
pub unsafe fn nvme_write(slba: u64, nlb: u16, prp1: PhysAddr) -> bool {
    NVME_DMA.submit_write(slba, nlb, prp1)
}

/// Sonde les compléments NVMe.
///
/// # Safety : contexte non-préemptible.
pub unsafe fn nvme_poll() -> usize {
    NVME_DMA.poll_cq()
}
