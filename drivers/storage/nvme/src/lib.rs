#![no_std]
//! exo-nvme — Driver NVMe (SSD) userspace pour Exo-OS.
//!
//! Driver **réel** conforme NVM Express base spec 1.4, écrit pour le microkernel
//! Exo-OS (les drivers tournent en Ring 3 ; le kernel fournit DMA + mapping MMIO
//! via [`NvmeHal`]). Présente la même interface bloc que `exo-virtio-blk`
//! (`read_block`/`write_block`/`flush` sur des blocs ExoFS de 4096 octets).
//!
//! ## Sûreté / anti-CVE
//! - I/O **synchrone** une-commande-à-la-fois : pas de course sur les anneaux.
//! - Indices d'anneau, phase tag, encodage de commande et bornes PRP isolés
//!   dans [`regs`], [`cmd`], [`queue`] et testés exhaustivement.
//! - Transferts bornés à 1 page (= 1 bloc ExoFS) → un seul PRP, jamais de PRP
//!   list mal formée.
//! - Toute attente matérielle est **bornée** (compteur de spins) → pas de hang.
//! - `dma_alloc` échoue proprement (fail-closed) plutôt que de fabriquer une
//!   adresse physique fictive.

extern crate alloc;

pub mod cmd;
pub mod queue;
pub mod regs;

use cmd::{cns, Completion, Sqe};
use queue::{CqRing, SqRing};

/// Taille de bloc ExoFS présentée en surface.
pub const EXOFS_BLOCK_SIZE: usize = 4096;
/// Taille des files I/O (entrées). Conservateur : tient en une page.
const IO_QUEUE_ENTRIES: u16 = 64;
const ADMIN_QUEUE_ENTRIES: u16 = 64;
const PAGE_SIZE: usize = 4096;
const IO_QID: u16 = 1;
/// Borne d'attente (spins) pour CSTS.RDY et complétions — anti-hang.
const SPIN_LIMIT: u32 = 50_000_000;

// ─────────────────────────────────────────────────────────────────────────────
// HAL — primitives fournies par le kernel (ou un mock en test)
// ─────────────────────────────────────────────────────────────────────────────

/// Région DMA contiguë : adresse physique (vue device) + virtuelle (vue driver).
#[derive(Clone, Copy, Debug)]
pub struct DmaRegion {
    pub phys: u64,
    pub virt: *mut u8,
    pub pages: usize,
}

/// Abstraction matérielle injectée par l'appelant. Le kernel l'implémente
/// au-dessus de son allocateur DMA et du mapping MMIO du BAR0 ; les tests
/// l'implémentent au-dessus du tas.
pub trait NvmeHal {
    fn dma_alloc(&self, pages: usize) -> Option<DmaRegion>;
    /// # Safety
    /// `region` doit provenir de `dma_alloc` et ne plus être référencée.
    unsafe fn dma_dealloc(&self, region: DmaRegion);
    fn mmio_read32(&self, off: usize) -> u32;
    fn mmio_write32(&self, off: usize, val: u32);
    fn mmio_read64(&self, off: usize) -> u64;
    fn mmio_write64(&self, off: usize, val: u64);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NvmeError {
    DmaExhausted,
    ControllerTimeout,
    ControllerFatal,
    AdminCommandFailed(u16),
    IoCommandFailed(u16),
    UnsupportedLbaSize(u32),
    InvalidBuffer,
    OutOfBounds,
    PrpUnsupported,
    NotInitialized,
}

// ─────────────────────────────────────────────────────────────────────────────
// Driver
// ─────────────────────────────────────────────────────────────────────────────

pub struct NvmeDevice<H: NvmeHal> {
    hal: H,
    doorbell_stride: usize,
    asq: DmaRegion,
    acq: DmaRegion,
    iosq: DmaRegion,
    iocq: DmaRegion,
    data: DmaRegion, // buffer rebond 1 page pour les transferts bloc
    admin_sq: SqRing,
    admin_cq: CqRing,
    io_sq: SqRing,
    io_cq: CqRing,
    next_cid: u16,
    nsid: u32,
    lba_size: u32,
    capacity_lba: u64,
}

impl<H: NvmeHal> NvmeDevice<H> {
    /// Initialise le contrôleur : reset, files admin, enable, files I/O, identify.
    pub fn new(hal: H) -> Result<Self, NvmeError> {
        let cap = hal.mmio_read64(regs::REG_CAP);
        let stride = regs::cap_doorbell_stride(cap);

        // Limiter la taille de file à ce que le contrôleur supporte ET à une page.
        let max_entries = regs::cap_max_queue_entries(cap).min(IO_QUEUE_ENTRIES as u32) as u16;
        let q_entries = max_entries.max(2);

        let asq = alloc_zeroed(&hal, 1)?;
        let acq = alloc_zeroed(&hal, 1)?;
        let iosq = alloc_zeroed(&hal, 1)?;
        let iocq = alloc_zeroed(&hal, 1)?;
        let data = alloc_zeroed(&hal, 1)?;

        let mut dev = Self {
            hal,
            doorbell_stride: stride,
            asq,
            acq,
            iosq,
            iocq,
            data,
            admin_sq: SqRing::new(ADMIN_QUEUE_ENTRIES.min(q_entries)),
            admin_cq: CqRing::new(ADMIN_QUEUE_ENTRIES.min(q_entries)),
            io_sq: SqRing::new(q_entries),
            io_cq: CqRing::new(q_entries),
            next_cid: 1,
            nsid: 1,
            lba_size: 512,
            capacity_lba: 0,
        };

        dev.reset_and_enable(cap)?;
        dev.create_io_queues()?;
        dev.identify_namespace()?;
        Ok(dev)
    }

    fn reset_and_enable(&mut self, cap: u64) -> Result<(), NvmeError> {
        // 1. Désactiver le contrôleur (CC.EN=0) et attendre CSTS.RDY=0.
        self.hal.mmio_write32(regs::REG_CC, 0);
        self.wait_ready(false)?;

        // 2. Programmer AQA / ASQ / ACQ.
        self.hal.mmio_write32(
            regs::REG_AQA,
            regs::aqa_value(self.admin_sq.size as u32, self.admin_cq.size as u32),
        );
        self.hal.mmio_write64(regs::REG_ASQ, self.asq.phys);
        self.hal.mmio_write64(regs::REG_ACQ, self.acq.phys);

        // 3. MPS = page 4 KiB (doit être >= CAP.MPSMIN). Activer.
        let mps = 0u32; // 2^(12+0) = 4096
        debug_assert!(regs::cap_mpsmin_shift(cap) <= 12 + mps);
        self.hal
            .mmio_write32(regs::REG_CC, regs::cc_value(true, mps));

        // 4. Attendre CSTS.RDY=1.
        self.wait_ready(true)
    }

    fn wait_ready(&self, target: bool) -> Result<(), NvmeError> {
        let mut spins = 0u32;
        loop {
            let csts = self.hal.mmio_read32(regs::REG_CSTS);
            if regs::csts_fatal(csts) {
                return Err(NvmeError::ControllerFatal);
            }
            if regs::csts_ready(csts) == target {
                return Ok(());
            }
            spins += 1;
            if spins >= SPIN_LIMIT {
                return Err(NvmeError::ControllerTimeout);
            }
            core::hint::spin_loop();
        }
    }

    fn alloc_cid(&mut self) -> u16 {
        let cid = self.next_cid;
        self.next_cid = self.next_cid.wrapping_add(1);
        // CID 0 réservé conventionnellement ; éviter de le réutiliser.
        if self.next_cid == 0 {
            self.next_cid = 1;
        }
        cid
    }

    fn create_io_queues(&mut self) -> Result<(), NvmeError> {
        // L'ordre impose : la CQ AVANT la SQ qui la référence.
        let cid = self.alloc_cid();
        let cqe = Sqe::create_io_cq(cid, IO_QID, self.io_cq.size, self.iocq.phys, false);
        self.submit_admin(cqe, cid)?;

        let cid = self.alloc_cid();
        let sqe = Sqe::create_io_sq(cid, IO_QID, self.io_sq.size, self.iosq.phys, IO_QID);
        self.submit_admin(sqe, cid)?;
        Ok(())
    }

    fn identify_namespace(&mut self) -> Result<(), NvmeError> {
        let cid = self.alloc_cid();
        let sqe = Sqe::identify(cid, self.nsid, cns::NAMESPACE, self.data.phys);
        self.submit_admin(sqe, cid)?;

        // Lire NSZE (octets 0-7) et FLBAS (octet 26) + LBAF.
        // SAFETY: `data` est une page DMA valide remplie par le contrôleur.
        let base = self.data.virt;
        let nsze = unsafe { read_u64(base, 0) };
        let flbas = unsafe { core::ptr::read_volatile(base.add(26)) } & 0x0F;
        let lbaf_off = 128 + (flbas as usize) * 4;
        let lbaf = unsafe { read_u32(base, lbaf_off) };
        let lbads = (lbaf >> 16) & 0xFF; // log2(taille LBA)
        let lba_size = 1u32 << lbads;
        if lba_size == 0 || EXOFS_BLOCK_SIZE % (lba_size as usize) != 0 {
            return Err(NvmeError::UnsupportedLbaSize(lba_size));
        }
        self.lba_size = lba_size;
        self.capacity_lba = nsze;
        Ok(())
    }

    /// Soumet une commande admin et attend (poll) sa complétion.
    fn submit_admin(&mut self, sqe: Sqe, expected_cid: u16) -> Result<Completion, NvmeError> {
        submit_and_poll(
            &self.hal,
            &self.asq,
            &self.acq,
            &mut self.admin_sq,
            &mut self.admin_cq,
            0,
            self.doorbell_stride,
            sqe,
            expected_cid,
        )
        .map_err(|status| NvmeError::AdminCommandFailed(status))
    }

    /// Soumet une commande I/O et attend sa complétion.
    fn submit_io(&mut self, sqe: Sqe, expected_cid: u16) -> Result<Completion, NvmeError> {
        submit_and_poll(
            &self.hal,
            &self.iosq,
            &self.iocq,
            &mut self.io_sq,
            &mut self.io_cq,
            IO_QID,
            self.doorbell_stride,
            sqe,
            expected_cid,
        )
        .map_err(|status| NvmeError::IoCommandFailed(status))
    }

    // ── Surface bloc ─────────────────────────────────────────────────────────

    pub fn block_size(&self) -> u32 {
        EXOFS_BLOCK_SIZE as u32
    }

    fn sectors_per_block(&self) -> u64 {
        (EXOFS_BLOCK_SIZE as u64) / (self.lba_size as u64)
    }

    pub fn total_blocks(&self) -> u64 {
        let spb = self.sectors_per_block();
        if spb == 0 {
            0
        } else {
            self.capacity_lba / spb
        }
    }

    fn block_to_slba(&self, block_id: u64) -> Result<(u64, u16), NvmeError> {
        if block_id >= self.total_blocks() {
            return Err(NvmeError::OutOfBounds);
        }
        let spb = self.sectors_per_block();
        let slba = block_id
            .checked_mul(spb)
            .ok_or(NvmeError::OutOfBounds)?;
        let nlb_zero_based = (spb - 1) as u16; // spb ∈ {1..8}, tient en u16
        Ok((slba, nlb_zero_based))
    }

    pub fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> Result<(), NvmeError> {
        if buf.len() != EXOFS_BLOCK_SIZE {
            return Err(NvmeError::InvalidBuffer);
        }
        let (slba, nlb) = self.block_to_slba(block_id)?;
        let (prp1, prp2) = cmd::build_prp(self.data.phys, EXOFS_BLOCK_SIZE, PAGE_SIZE)
            .ok_or(NvmeError::PrpUnsupported)?;
        let cid = self.alloc_cid();
        let sqe = Sqe::read_write(false, cid, self.nsid, slba, nlb, prp1, prp2);
        self.submit_io(sqe, cid)?;
        // Copier le buffer rebond DMA vers l'appelant.
        // SAFETY: `data` est une page DMA valide de 4096 octets remplie par le read.
        unsafe {
            core::ptr::copy_nonoverlapping(self.data.virt, buf.as_mut_ptr(), EXOFS_BLOCK_SIZE);
        }
        Ok(())
    }

    pub fn write_block(&mut self, block_id: u64, buf: &[u8]) -> Result<(), NvmeError> {
        if buf.len() != EXOFS_BLOCK_SIZE {
            return Err(NvmeError::InvalidBuffer);
        }
        let (slba, nlb) = self.block_to_slba(block_id)?;
        // Copier l'appelant vers le buffer rebond DMA.
        // SAFETY: `data` est une page DMA valide de 4096 octets.
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), self.data.virt, EXOFS_BLOCK_SIZE);
        }
        let (prp1, prp2) = cmd::build_prp(self.data.phys, EXOFS_BLOCK_SIZE, PAGE_SIZE)
            .ok_or(NvmeError::PrpUnsupported)?;
        let cid = self.alloc_cid();
        let sqe = Sqe::read_write(true, cid, self.nsid, slba, nlb, prp1, prp2);
        self.submit_io(sqe, cid)?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), NvmeError> {
        let cid = self.alloc_cid();
        let mut sqe = Sqe::zeroed();
        sqe.dword[0] = cmd::nvm::FLUSH as u32 | ((cid as u32) << 16);
        sqe.dword[1] = self.nsid;
        self.submit_io(sqe, cid)?;
        Ok(())
    }
}

impl<H: NvmeHal> Drop for NvmeDevice<H> {
    fn drop(&mut self) {
        // Désactiver le contrôleur pour stopper tout accès DMA en vol.
        self.hal.mmio_write32(regs::REG_CC, 0);
        // SAFETY: les régions proviennent de dma_alloc et ne sont plus utilisées.
        unsafe {
            self.hal.dma_dealloc(self.asq);
            self.hal.dma_dealloc(self.acq);
            self.hal.dma_dealloc(self.iosq);
            self.hal.dma_dealloc(self.iocq);
            self.hal.dma_dealloc(self.data);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn alloc_zeroed<H: NvmeHal>(hal: &H, pages: usize) -> Result<DmaRegion, NvmeError> {
    let region = hal.dma_alloc(pages).ok_or(NvmeError::DmaExhausted)?;
    // SAFETY: dma_alloc garantit `pages * PAGE_SIZE` octets valides à `virt`.
    unsafe {
        core::ptr::write_bytes(region.virt, 0, pages * PAGE_SIZE);
    }
    Ok(region)
}

#[inline]
unsafe fn read_u32(base: *const u8, off: usize) -> u32 {
    let mut v = [0u8; 4];
    for (i, b) in v.iter_mut().enumerate() {
        *b = core::ptr::read_volatile(base.add(off + i));
    }
    u32::from_le_bytes(v)
}

#[inline]
unsafe fn read_u64(base: *const u8, off: usize) -> u64 {
    let mut v = [0u8; 8];
    for (i, b) in v.iter_mut().enumerate() {
        *b = core::ptr::read_volatile(base.add(off + i));
    }
    u64::from_le_bytes(v)
}

/// Écrit `sqe` dans la SQ au tail courant, sonne le doorbell, puis poll la CQ
/// jusqu'à la complétion du `expected_cid`. Retourne `Err(status)` si échec.
#[allow(clippy::too_many_arguments)]
fn submit_and_poll<H: NvmeHal>(
    hal: &H,
    sq: &DmaRegion,
    cq: &DmaRegion,
    sq_ring: &mut SqRing,
    cq_ring: &mut CqRing,
    qid: u16,
    stride: usize,
    sqe: Sqe,
    expected_cid: u16,
) -> Result<Completion, u16> {
    // 1. Écrire l'entrée à l'index tail.
    let slot = sq_ring.tail() as usize;
    // SAFETY: `slot < sq.size` (anneau) et la SQ fait au moins size*64 octets.
    unsafe {
        let dst = (sq.virt as *mut Sqe).add(slot);
        core::ptr::write_volatile(dst, sqe);
    }
    // 2. Avancer le tail + sonner le SQ tail doorbell.
    let new_tail = sq_ring.advance();
    hal.mmio_write32(
        regs::sq_tail_doorbell(qid as u32, stride),
        new_tail as u32,
    );

    // 3. Poll la CQ à head jusqu'à une entrée neuve (phase tag).
    let mut spins = 0u32;
    loop {
        let head = cq_ring.head() as usize;
        // SAFETY: head < cq.size ; chaque entrée fait 16 octets.
        let dw = unsafe {
            let p = (cq.virt as *const u32).add(head * 4);
            [
                core::ptr::read_volatile(p),
                core::ptr::read_volatile(p.add(1)),
                core::ptr::read_volatile(p.add(2)),
                core::ptr::read_volatile(p.add(3)),
            ]
        };
        let c = Completion::from_dwords(dw);
        if cq_ring.entry_is_new(c.phase) {
            // Consommer : avancer head + sonner CQ head doorbell.
            let new_head = cq_ring.advance();
            hal.mmio_write32(
                regs::cq_head_doorbell(qid as u32, stride),
                new_head as u32,
            );
            if c.cid != expected_cid {
                // Complétion d'une autre commande : en I/O synchrone, ne devrait
                // pas arriver. On la traite comme une erreur de protocole.
                return Err(0xFFFF);
            }
            if c.is_success() {
                return Ok(c);
            }
            return Err(c.status);
        }
        spins += 1;
        if spins >= SPIN_LIMIT {
            return Err(0xFFFE); // timeout
        }
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests;
