// kernel/src/memory/dma/engines/virtio_dma.rs
//
// Moteur DMA VirtIO (virtqueue split ring, spécification VirtIO 1.1, §2.6).
// Couche 0 — no_std, aucun heap, statique uniquement.
#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::PhysAddr;
use crate::memory::dma::stats::counters::{DMA_STATS, dma_stat_submit, dma_stat_complete, dma_stat_error};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES VIRTIO
// ─────────────────────────────────────────────────────────────────────────────

/// Taille de la virtqueue (doit être une puissance de 2, max 32768).
pub const VIRTQ_SIZE: usize = 256;

// Flags de descripteur (VirtIO §2.6.5).
mod desc_flags {
    /// Descripteur en lecture seule pour le device (write = 0 par défaut).
    pub const NEXT:     u16 = 1 << 0;  // Chaîn next.
    pub const WRITE:    u16 = 1 << 1;  // Device écrit (sinon device lit).
    pub const INDIRECT: u16 = 1 << 2;  // Descripteur de table indirecte.
}

// Flags de l'available ring.
mod avail_flags {
    pub const NO_INTERRUPT: u16 = 1;
}

// ─────────────────────────────────────────────────────────────────────────────
// STRUCTURES VIRTQUEUE SPLIT RING
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur de virtqueue (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VirtqDesc {
    /// Adresse physique du buffer.
    pub addr:  u64,
    /// Longueur du buffer.
    pub len:   u32,
    /// Flags (desc_flags).
    pub flags: u16,
    /// Index du prochain descripteur (si NEXT).
    pub next:  u16,
}

/// Available Ring (place des buffers disponibles pour le device).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx:   u16,
    pub ring:  [u16; VIRTQ_SIZE],
    pub used_event: u16,
}

/// Élément du Used Ring.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtqUsedElem {
    /// Index du descripteur de tête.
    pub id:  u32,
    /// Nombre d'octets écrits par le device.
    pub len: u32,
}

/// Used Ring (retour du device).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx:   u16,
    pub ring:  [VirtqUsedElem; VIRTQ_SIZE],
    pub avail_event: u16,
}

// ─────────────────────────────────────────────────────────────────────────────
// VIRTQUEUE INTERNE
// ─────────────────────────────────────────────────────────────────────────────

/// État interne de la virtqueue.
struct VirtqState {
    desc:        [VirtqDesc; VIRTQ_SIZE],
    avail:       VirtqAvail,
    used:        VirtqUsed,
    /// Bitmap des descripteurs libres (bit=1 → libre).
    free_bitmap: [u64; VIRTQ_SIZE / 64],
    /// Indice Our last_used_idx pour la sonde de la used ring.
    last_used:   u16,
    /// Taille de données par slot (pour les stats).
    slot_bytes:  [u32; VIRTQ_SIZE],
    slot_write:  [bool; VIRTQ_SIZE],
}

impl VirtqState {
    fn new() -> Self {
        let avail = VirtqAvail {
            flags: 0, idx: 0,
            ring: [0; VIRTQ_SIZE],
            used_event: 0,
        };
        let used = VirtqUsed {
            flags: 0, idx: 0,
            ring: [VirtqUsedElem::default(); VIRTQ_SIZE],
            avail_event: 0,
        };
        VirtqState {
            desc:        [VirtqDesc::default(); VIRTQ_SIZE],
            avail,
            used,
            free_bitmap: [!0u64; VIRTQ_SIZE / 64],   // tous libres
            last_used:   0,
            slot_bytes:  [0; VIRTQ_SIZE],
            slot_write:  [false; VIRTQ_SIZE],
        }
    }

    /// Alloue un index de descripteur libre. Retourne None si la virtqueue est pleine.
    fn alloc_desc(&mut self) -> Option<u16> {
        for (i, word) in self.free_bitmap.iter_mut().enumerate() {
            if *word != 0 {
                let bit = (*word).trailing_zeros() as usize;
                *word &= !( 1u64 << bit );
                return Some((i * 64 + bit) as u16);
            }
        }
        None
    }

    /// Libère un index de descripteur.
    fn free_desc(&mut self, idx: u16) {
        let i = (idx as usize) / 64;
        let b = (idx as usize) % 64;
        self.free_bitmap[i] |= 1u64 << b;
    }

    /// Soumet un buffer (1 descripteur → addresse physique + longueur).
    ///
    /// `write` = le device écrit dans ce buffer (lecture du point de vue host).
    fn submit_buf(&mut self, phys: PhysAddr, len: u32, write: bool) -> Option<u16> {
        let idx = self.alloc_desc()?;
        self.desc[idx as usize] = VirtqDesc {
            addr:  phys.as_u64(),
            len,
            flags: if write { desc_flags::WRITE } else { 0 },
            next:  0,
        };
        self.slot_bytes[idx as usize] = len;
        self.slot_write[idx as usize] = write;

        // Placer dans l'available ring.
        let ai = (self.avail.idx as usize) % VIRTQ_SIZE;
        self.avail.ring[ai] = idx;
        // Mémoire ordering : update idx après le ring.
        core::sync::atomic::fence(Ordering::Release);
        self.avail.idx = self.avail.idx.wrapping_add(1);
        Some(idx)
    }

    /// Vérifie et consomme les entrées du used ring.
    fn poll_used(&mut self) -> (usize, usize, usize) {
        let mut ok = 0usize;
        let mut err = 0usize;
        let mut total_bytes = 0usize;

        // Ordering acquire pour voir les writes du device.
        core::sync::atomic::fence(Ordering::Acquire);
        while self.last_used != self.used.idx {
            let ui = (self.last_used as usize) % VIRTQ_SIZE;
            let elem = self.used.ring[ui];
            self.last_used = self.last_used.wrapping_add(1);
            let desc_idx = elem.id as u16;
            // len == 0 peut indiquer une erreur selon le device.
            if elem.len > 0 || !self.slot_write[desc_idx as usize] {
                ok += 1;
                total_bytes += elem.len as usize;
            } else {
                err += 1;
            }
            self.free_desc(desc_idx);
        }
        (ok, err, total_bytes)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MMIO VIRTIO
// ─────────────────────────────────────────────────────────────────────────────

mod virtio_mmio {
    /// Magic Value (0x74726976 = "virt").
    pub const MAGIC:         usize = 0x000;
    /// Device ID.
    pub const DEVICE_ID:     usize = 0x008;
    /// Vendor ID.
    pub const VENDOR_ID:     usize = 0x00C;
    /// Device Feature Bits.
    pub const DEVICE_FEAT:   usize = 0x010;
    /// Driver Feature Bits.
    pub const DRIVER_FEAT:   usize = 0x020;
    /// Queue Selector.
    pub const QUEUE_SEL:     usize = 0x030;
    /// Queue Max (read).
    pub const QUEUE_NUM_MAX: usize = 0x034;
    /// Queue Size (write).
    pub const QUEUE_NUM:     usize = 0x038;
    /// Queue Alignment.
    pub const QUEUE_ALIGN:   usize = 0x03C;
    /// Queue PFN.
    pub const QUEUE_PFN:     usize = 0x040;
    /// Queue Ready (MMIO v2).
    pub const QUEUE_READY:   usize = 0x044;
    /// Queue Notify.
    pub const QUEUE_NOTIFY:  usize = 0x050;
    /// Status.
    pub const STATUS:        usize = 0x070;
}

/// Bits de status (§3.1).
mod status {
    pub const ACKNOWLEDGE: u32 = 1;
    pub const DRIVER:      u32 = 2;
    pub const DRIVER_OK:   u32 = 4;
    pub const FEATURES_OK: u32 = 8;
    pub const FAILED:      u32 = 128;
}

// ─────────────────────────────────────────────────────────────────────────────
// MOTEUR VIRTIO DMA
// ─────────────────────────────────────────────────────────────────────────────

pub struct VirtioDmaEngine {
    mmio_base: AtomicU64,
    present:   AtomicBool,
    engine_id: AtomicU32,
    state:     Mutex<VirtqState>,
}

impl VirtioDmaEngine {
    const fn new_uninit() -> Self {
        VirtioDmaEngine {
            mmio_base: AtomicU64::new(0),
            present:   AtomicBool::new(false),
            engine_id: AtomicU32::new(0),
            state:     Mutex::new(unsafe { core::mem::zeroed() }),
        }
    }

    unsafe fn read32(&self, off: usize) -> u32 {
        ((self.mmio_base.load(Ordering::Relaxed) + off as u64) as *const u32).read_volatile()
    }

    unsafe fn write32(&self, off: usize, v: u32) {
        ((self.mmio_base.load(Ordering::Relaxed) + off as u64) as *mut u32).write_volatile(v);
    }

    /// Initialise le périphérique VirtIO MMIO.
    ///
    /// # Safety : `mmio_base` identité-mappé en CPL 0.
    pub unsafe fn init(&self, mmio_base: u64) -> bool {
        // Vérifier magic value.
        let magic = ((mmio_base + virtio_mmio::MAGIC as u64) as *const u32).read_volatile();
        if magic != 0x74726976 { return false; }

        self.mmio_base.store(mmio_base, Ordering::Release);

        // Séquence d'initialisation §3.1.
        self.write32(virtio_mmio::STATUS, 0);  // Reset.
        self.write32(virtio_mmio::STATUS, status::ACKNOWLEDGE);
        self.write32(virtio_mmio::STATUS, status::ACKNOWLEDGE | status::DRIVER);
        // Pas de feature negotiation ici (pas de supported feature bits BLK spécifiques).
        self.write32(virtio_mmio::STATUS, status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK);

        // Configurer la queue 0.
        self.write32(virtio_mmio::QUEUE_SEL, 0);
        let qmax = self.read32(virtio_mmio::QUEUE_NUM_MAX);
        if qmax == 0 { return false; }
        let qsize = (VIRTQ_SIZE as u32).min(qmax);
        self.write32(virtio_mmio::QUEUE_NUM, qsize);

        // Adresse physique du descriptor table (identité-mappé).
        let st = self.state.lock();
        let desc_phys = &st.desc as *const _ as u64;
        drop(st);

        // PFN = adresse physique / 4096.
        self.write32(virtio_mmio::QUEUE_PFN, (desc_phys >> 12) as u32);
        self.write32(virtio_mmio::QUEUE_ALIGN, 4096);

        // DRIVER_OK.
        self.write32(virtio_mmio::STATUS,
            status::ACKNOWLEDGE | status::DRIVER | status::FEATURES_OK | status::DRIVER_OK,
        );

        let eid = DMA_STATS.register_engine();
        self.engine_id.store(eid as u32, Ordering::Relaxed);
        self.present.store(true, Ordering::Release);
        true
    }

    /// Soumet un buffer pour le device (transfer DMA).
    ///
    /// `write` = vrai si le device doit écrire dans `phys` (lecture côté host).
    ///
    /// # Safety : `phys` valide pour toute la durée du transfert.
    pub unsafe fn submit(&self, phys: PhysAddr, len: u32, write: bool) -> bool {
        if !self.present.load(Ordering::Acquire) { return false; }
        let mut st = self.state.lock();
        let ok = st.submit_buf(phys, len, write).is_some();
        drop(st);
        if ok {
            // Notifier le device (queue 0).
            self.write32(virtio_mmio::QUEUE_NOTIFY, 0);
            let eid = self.engine_id.load(Ordering::Relaxed) as usize;
            dma_stat_submit(eid);
        }
        ok
    }

    /// Sonde le used ring.
    ///
    /// # Safety : contexte non-préemptible.
    pub unsafe fn poll(&self) -> usize {
        if !self.present.load(Ordering::Acquire) { return 0; }
        let mut st = self.state.lock();
        let (ok, err, bytes) = st.poll_used();
        drop(st);
        let eid = self.engine_id.load(Ordering::Relaxed) as usize;
        for _ in 0..ok {
            dma_stat_complete(eid, (bytes / ok.max(1)) as u64, true, 0);
        }
        for _ in 0..err {
            dma_stat_error(eid);
        }
        ok + err
    }
}

// SAFETY: VirtioDmaEngine utilise Mutex + atomics.
unsafe impl Sync for VirtioDmaEngine {}

/// Instance globale du moteur VirtIO DMA.
pub static VIRTIO_DMA: VirtioDmaEngine = VirtioDmaEngine::new_uninit();

/// Initialise le moteur VirtIO DMA.
///
/// # Safety : MMIO valide.
pub unsafe fn virtio_dma_init(mmio_base: u64) -> bool {
    VIRTIO_DMA.init(mmio_base)
}

/// Soumet un buffer VirtIO.
///
/// # Safety : `phys` valide.
pub unsafe fn virtio_dma_submit(phys: PhysAddr, len: u32, write: bool) -> bool {
    VIRTIO_DMA.submit(phys, len, write)
}

/// Sonde les compléments VirtIO.
///
/// # Safety : contexte non-préemptible.
pub unsafe fn virtio_dma_poll() -> usize {
    VIRTIO_DMA.poll()
}
