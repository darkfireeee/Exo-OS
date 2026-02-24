// kernel/src/memory/dma/engines/ioat.rs
//
// Intel I/OAT DMA Engine (Crystal Beach / Xeon series).
// Ref : Intel I/OAT DMA Engine User Guide, Doc #322931.
// Couche 0 — no_std kernel, accès MMIO via raw pointers.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::{PhysAddr, PAGE_SIZE};
use crate::memory::dma::stats::counters::{DMA_STATS, dma_stat_submit, dma_stat_complete, dma_stat_error};

// ─────────────────────────────────────────────────────────────────────────────
// REGISTRES MMIO IOAT (offset depuis MMIO base)
// ─────────────────────────────────────────────────────────────────────────────

/// Offsets des registres I/OAT channel 0 (CB3+).
mod regs {
    pub const CHANCMD:        usize = 0x04;  // Channel Command (u8)
    pub const XFERCAP:        usize = 0x06;  // Transfer Capability (u8)
    pub const GENCTRL:        usize = 0x08;  // General Control (u8)
    pub const INTRCTRL:       usize = 0x0C;  // Interrupt Control (u16)
    pub const ATTNSTATUS:     usize = 0x10;  // Attention Status (u32)
    pub const CHANCTRL:       usize = 0x80;  // Channel Control (u16) — per channel
    pub const DMACOUNT:       usize = 0x84;  // DMA Transfer Count (u16)
    pub const CHANSTS_LO:     usize = 0x88;  // Channel Status Low (u32)
    pub const CHANSTS_HI:     usize = 0x8C;  // Channel Status High (u32)
    pub const CHAINADDR_LO:   usize = 0x90;  // Descriptor Chain Address Low (u32)
    pub const CHAINADDR_HI:   usize = 0x94;  // Descriptor Chain Address High (u32)
    pub const CHANCMP_LO:     usize = 0x98;  // Completion Buffer Address Low (u32)
    pub const CHANCMP_HI:     usize = 0x9C;  // Completion Buffer Address High (u32)
    pub const DCACTRL:        usize = 0xB0;  // DCA Control (u32)
}

/// Bits du Channel Status Register.
mod chansts {
    pub const ACTIVE:    u64 = 0x0; // Canal actif (bits 4:0)
    pub const DONE:      u64 = 0x1; // Dernier descripteur complété
    pub const SUSPENDED: u64 = 0x2; // Canal suspendu
    pub const HALTED:    u64 = 0x3; // Erreur fatale
    pub const ARMED:     u64 = 0x4; // En attente d'un descripteur
    pub const STATUS_MASK: u64 = 0x7;
}

/// Bits du Channel Control Register.
mod chanctrl {
    pub const INT_DISABLE: u16 = 1 << 0;  // Désactive les interruptions
    pub const ERR_INT:     u16 = 1 << 2;  // Erreur → interruption
    pub const ANY_ERR_ABORT: u16 = 1 << 3; // Annule en cas d'erreur
}

// ─────────────────────────────────────────────────────────────────────────────
// DESCRIPTEUR I/OAT (CB3)
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur I/OAT DMA Engine version CB3 (DMA Copy).
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct IoatDescriptor {
    /// Longueur du transfert en octets (bits 0:23), flags (bits 24:31).
    pub size_flags: u32,
    /// Adresse physique destination (64 bits).
    pub dst_lo:     u32,
    pub dst_hi:     u32,
    /// Adresse physique source (64 bits).
    pub src_lo:     u32,
    pub src_hi:     u32,
    /// Adresse du descripteur suivant (NULL = fin de liste).
    pub next_lo:    u32,
    pub next_hi:    u32,
    /// Adresse du completion record (64 bits).
    pub cmp_lo:     u32,
    pub cmp_hi:     u32,
    /// Flags de contrôle supplémentaires (CB3: reserved).
    pub ctrl_result: u32,
    pub reserved:    u32,
    pub user_data:   u64,
}

impl IoatDescriptor {
    pub const FLAG_INT_EN:    u32 = 1 << 24; // Interrupt on completion
    pub const FLAG_DST_SNOOP: u32 = 1 << 25; // Destination snooping
    pub const FLAG_SRC_SNOOP: u32 = 1 << 26; // Source snooping
    pub const FLAG_CMP_ADDR:  u32 = 1 << 27; // Write completion address
    pub const FLAG_FENCE:     u32 = 1 << 28; // Memory fence après copie
    pub const FLAG_NULL:      u32 = 1 << 31; // Descripteur nul (NOP)

    /// Crée un descripteur de copie DMA.
    pub fn new_copy(src: PhysAddr, dst: PhysAddr, size: u32) -> Self {
        IoatDescriptor {
            size_flags:  size & 0x00FF_FFFF,  // bits 23:0 = taille
            dst_lo:      dst.as_u64() as u32,
            dst_hi:      (dst.as_u64() >> 32) as u32,
            src_lo:      src.as_u64() as u32,
            src_hi:      (src.as_u64() >> 32) as u32,
            next_lo:     0,
            next_hi:     0,
            cmp_lo:      0,
            cmp_hi:      0,
            ctrl_result: 0,
            reserved:    0,
            user_data:   0,
        }
    }

    /// Chaîne ce descripteur au suivant.
    pub fn set_next(&mut self, next_phys: PhysAddr) {
        self.next_lo = next_phys.as_u64() as u32;
        self.next_hi = (next_phys.as_u64() >> 32) as u32;
    }

    /// Définit l'adresse de completion record.
    pub fn set_completion_addr(&mut self, cmp_phys: PhysAddr) {
        self.cmp_lo = cmp_phys.as_u64() as u32;
        self.cmp_hi = (cmp_phys.as_u64() >> 32) as u32;
        self.size_flags |= Self::FLAG_CMP_ADDR;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RING DE DESCRIPTEURS IOAT
// ─────────────────────────────────────────────────────────────────────────────

/// Taille du ring de descripteurs I/OAT (doit être une puissance de 2).
pub const IOAT_RING_SIZE: usize = 256;

/// Ring de descripteurs I/OAT.
struct IoatRing {
    descs:   [IoatDescriptor; IOAT_RING_SIZE],
    /// Completion records (un par slot, 64-bit aligned).
    cmp:     [u64; IOAT_RING_SIZE],
    head:    u16,  // prochain slot à écrire
    tail:    u16,  // prochain slot à compléter
    count:   u16,  // descripteurs in-flight
}

impl IoatRing {
    const fn new() -> Self {
        IoatRing {
            descs:  [IoatDescriptor {
                size_flags: 0, dst_lo: 0, dst_hi: 0, src_lo: 0, src_hi: 0,
                next_lo: 0, next_hi: 0, cmp_lo: 0, cmp_hi: 0,
                ctrl_result: 0, reserved: 0, user_data: 0,
            }; IOAT_RING_SIZE],
            cmp:   [0u64; IOAT_RING_SIZE],
            head:  0,
            tail:  0,
            count: 0,
        }
    }

    fn available(&self) -> usize {
        IOAT_RING_SIZE - self.count as usize - 1
    }

    fn is_full(&self) -> bool {
        self.count as usize >= IOAT_RING_SIZE - 1
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MOTEUR IOAT
// ─────────────────────────────────────────────────────────────────────────────

/// Moteur I/OAT DMA.
pub struct IoatEngine {
    /// Base MMIO du canal 0.
    mmio_base:   AtomicU64,
    /// Moteur présent et initialisé.
    present:     AtomicBool,
    /// Identifiant stats.
    engine_id:   AtomicU32,
    /// Ring de descripteurs.
    ring:        Mutex<IoatRing>,
}

impl IoatEngine {
    const fn new() -> Self {
        IoatEngine {
            mmio_base: AtomicU64::new(0),
            present:   AtomicBool::new(false),
            engine_id: AtomicU32::new(0),
            ring:      Mutex::new(IoatRing::new()),
        }
    }

    /// Lit un registre 8 bits.
    ///
    /// # Safety : `mmio_base` doit être l'adresse MMIO valide et mappée.
    unsafe fn read8(&self, offset: usize) -> u8 {
        let base = self.mmio_base.load(Ordering::Relaxed) as *const u8;
        base.add(offset).read_volatile()
    }

    /// Lit un registre 16 bits.
    unsafe fn read16(&self, offset: usize) -> u16 {
        let base = self.mmio_base.load(Ordering::Relaxed) as *const u16;
        base.add(offset / 2).read_volatile()
    }

    /// Lit un registre 32 bits.
    unsafe fn read32(&self, offset: usize) -> u32 {
        let base = self.mmio_base.load(Ordering::Relaxed) as *const u32;
        base.add(offset / 4).read_volatile()
    }

    /// Écrit un registre 16 bits.
    unsafe fn write16(&self, offset: usize, val: u16) {
        let base = self.mmio_base.load(Ordering::Relaxed) as *mut u16;
        base.add(offset / 2).write_volatile(val);
    }

    /// Écrit un registre 32 bits.
    unsafe fn write32(&self, offset: usize, val: u32) {
        let base = self.mmio_base.load(Ordering::Relaxed) as *mut u32;
        base.add(offset / 4).write_volatile(val);
    }

    /// Lit le statut du canal.
    unsafe fn chan_status(&self) -> u64 {
        let lo = self.read32(regs::CHANSTS_LO) as u64;
        let hi = self.read32(regs::CHANSTS_HI) as u64;
        lo | (hi << 32)
    }

    /// Initialise le canal I/OAT.
    ///
    /// # Safety : CPL 0, `mmio_base` valide et mappée en WB.
    pub unsafe fn init(&self, mmio_base: u64) -> bool {
        self.mmio_base.store(mmio_base, Ordering::Release);

        // 1. Lire la capacité de transfert (XFERCAP).
        let xfercap = self.read8(regs::XFERCAP);
        if xfercap == 0 { return false; }  // Contrôleur absent ou invalid

        // 2. Désactiver les interruptions, activer l'arrêt sur erreur.
        let ctrl = chanctrl::INT_DISABLE | chanctrl::ANY_ERR_ABORT;
        self.write16(regs::CHANCTRL, ctrl);

        // 3. Réinitialiser le canal (RESET = 0x20 dans CHANCMD).
        let cmd_offs = regs::CHANCMD as usize;
        let base = mmio_base as *mut u8;
        base.add(cmd_offs).write_volatile(0x20);  // CHANCMD_RESET

        // 4. Attendre la fin du reset (max 1000 lectures).
        for _ in 0..1000 {
            let status = self.chan_status() & chansts::STATUS_MASK;
            if status != chansts::HALTED { break; }
        }

        let engine_id = DMA_STATS.register_engine();
        self.engine_id.store(engine_id as u32, Ordering::Relaxed);
        self.present.store(true, Ordering::Release);
        true
    }

    /// Soumet une copie DMA.
    ///
    /// `src` / `dst` : adresses physiques. `size` en octets (≤ 4 MiB par desc.).
    /// Retourne l'index du slot ou None si ring plein.
    ///
    /// # Safety : adresses physiques valides, mappées en physmap.
    pub unsafe fn submit_copy(&self, src: PhysAddr, dst: PhysAddr, size: u32) -> Option<u16> {
        if !self.present.load(Ordering::Acquire) { return None; }
        let mut ring = self.ring.lock();
        if ring.is_full() { return None; }

        let slot = ring.head;
        let next_slot = (slot + 1) as usize % IOAT_RING_SIZE;

        // Construire le descripteur.
        ring.descs[slot as usize] = IoatDescriptor::new_copy(src, dst, size);

        // Adresse physique du completion record pour ce slot.
        // En production, les records sont dans une page physique dédiée allouée à l'init.
        // Ici : utiliser l'adresse du champ cmp[] via physmap — simplifié pour init.
        ring.count += 1;
        ring.head   = next_slot as u16;

        // Bump DMACOUNT pour déclencher le moteur (registre 16 bits du nombre de descs).
        self.write16(regs::DMACOUNT, ring.count);

        let eid = self.engine_id.load(Ordering::Relaxed) as usize;
        dma_stat_submit(eid);

        Some(slot)
    }

    /// Sonde les compléments — retourne le nombre de transferts terminés.
    ///
    /// # Safety : appelé depuis un contexte non-préemptible (IRQ off ou spinlock).
    pub unsafe fn poll(&self) -> usize {
        if !self.present.load(Ordering::Acquire) { return 0; }
        let status = self.chan_status();
        let state  = status & chansts::STATUS_MASK;
        let eid    = self.engine_id.load(Ordering::Relaxed) as usize;

        if state == chansts::DONE {
            let mut ring = self.ring.lock();
            let completed = ring.count as usize;
            ring.count = 0;
            ring.tail  = ring.head;
            drop(ring);
            dma_stat_complete(eid, 0, true, 0);
            return completed;
        }

        if state == chansts::HALTED {
            // Erreur fatale — reset du canal.
            let base = self.mmio_base.load(Ordering::Relaxed) as *mut u8;
            base.add(regs::CHANCMD).write_volatile(0x20); // RESET
            let mut ring = self.ring.lock();
            let failed = ring.count as usize;
            ring.count = 0;
            ring.tail  = ring.head;
            drop(ring);
            dma_stat_error(eid);
            let _ = failed;
        }
        0
    }
}

// SAFETY: IoatEngine utilise un Mutex interne et des accès atomiques.
unsafe impl Sync for IoatEngine {}

/// Instance globale du moteur I/OAT.
pub static IOAT_ENGINE: IoatEngine = IoatEngine::new();

/// Initialise le moteur I/OAT.
///
/// # Safety : `mmio_base` = base du BAR MMIO PCI du contrôleur I/OAT.
pub unsafe fn ioat_init(mmio_base: u64) -> bool {
    IOAT_ENGINE.init(mmio_base)
}

/// Soumet une copie DMA via I/OAT.
///
/// # Safety : adresses physiques valides.
pub unsafe fn ioat_submit(src: PhysAddr, dst: PhysAddr, size: u32) -> Option<u16> {
    IOAT_ENGINE.submit_copy(src, dst, size)
}

/// Sonde les compléments I/OAT.
///
/// # Safety : contexte non-préemptible.
pub unsafe fn ioat_poll() -> usize {
    IOAT_ENGINE.poll()
}
