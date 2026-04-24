// kernel/src/memory/swap/backend.rs
//
// Backend de swap — gère les dispositifs de swap (partition ou fichier).
// Couche 0 — aucune dépendance vers fs/process/scheduler.
// L'intégration avec le FS se fait via le trait `SwapDevice`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::memory::core::types::PhysAddr;

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT D'ABSTRACTION DU DISPOSITIF SWAP
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant d'un slot swap (index d'une page dans le swap).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct SwapSlot(pub u64);

impl SwapSlot {
    pub const INVALID: Self = SwapSlot(u64::MAX);
    #[inline]
    pub fn is_valid(self) -> bool {
        self.0 != u64::MAX
    }
    #[inline]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Trait d'abstraction d'un dispositif swap.
/// Implémenté par le sous-système FS/block layer, mais défini ici (Couche 0).
pub trait SwapDevice: Send + Sync {
    /// Lit une page swap depuis le dispositif vers la mémoire physique `dst`.
    ///
    /// # Safety
    /// `dst` doit être un frame physique valide.
    unsafe fn read_page(&self, slot: SwapSlot, dst: PhysAddr) -> Result<(), SwapError>;

    /// Écrit une page swap depuis la mémoire physique `src` vers le dispositif.
    ///
    /// # Safety
    /// `src` doit être un frame physique valide.
    unsafe fn write_page(&self, slot: SwapSlot, src: PhysAddr) -> Result<(), SwapError>;

    /// Alloue un slot libre dans le dispositif.
    fn alloc_slot(&self) -> Option<SwapSlot>;

    /// Libère un slot (la page a été rechargée définitivement en RAM).
    fn free_slot(&self, slot: SwapSlot);

    /// Retourne la capacité totale en pages.
    fn capacity_pages(&self) -> u64;

    /// Retourne le nombre de pages libres.
    fn free_pages(&self) -> u64;
}

// ─────────────────────────────────────────────────────────────────────────────
// ERREURS SWAP
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum SwapError {
    NoDevice = 0,
    NoSlot = 1,
    IoError = 2,
    InvalidSlot = 3,
    DeviceFull = 4,
    NotEnabled = 5,
    Corrupted = 6,
}

// ─────────────────────────────────────────────────────────────────────────────
// REGISTRE DES DISPOSITIFS SWAP
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_SWAP_DEVICES: usize = 8;

/// Table des dispositifs swap enregistrés.
pub struct SwapBackendRegistry {
    devices: [Option<&'static dyn SwapDevice>; MAX_SWAP_DEVICES],
    count: AtomicU32,
    enabled: AtomicBool,
    // Statistiques globales.
    pub stats_reads: AtomicU64,
    pub stats_writes: AtomicU64,
    pub stats_errors: AtomicU64,
    pub stats_slots_freed: AtomicU64,
}

// SAFETY: devices est protected by AtomicU32 (write-once per slot during init).
unsafe impl Sync for SwapBackendRegistry {}
unsafe impl Send for SwapBackendRegistry {}

impl SwapBackendRegistry {
    const fn new() -> Self {
        SwapBackendRegistry {
            devices: [None; MAX_SWAP_DEVICES],
            count: AtomicU32::new(0),
            enabled: AtomicBool::new(false),
            stats_reads: AtomicU64::new(0),
            stats_writes: AtomicU64::new(0),
            stats_errors: AtomicU64::new(0),
            stats_slots_freed: AtomicU64::new(0),
        }
    }

    /// Enregistre un dispositif swap.
    ///
    /// # Safety
    /// `device` doit avoir une durée de vie statique.
    pub unsafe fn register(&self, device: &'static dyn SwapDevice) -> Result<(), SwapError> {
        let idx = self.count.load(Ordering::Relaxed) as usize;
        if idx >= MAX_SWAP_DEVICES {
            return Err(SwapError::DeviceFull);
        }
        // SAFETY: addr_of! évite &T→*mut T ; write-once par slot garanti par count atomique.
        let ptr = core::ptr::addr_of!(self.devices[idx]) as *mut Option<&'static dyn SwapDevice>;
        core::ptr::write(ptr, Some(device));
        self.count.fetch_add(1, Ordering::Release);
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Alloue un slot sur le premier dispositif ayant de l'espace.
    pub fn alloc_slot(&self) -> Result<(usize, SwapSlot), SwapError> {
        let count = self.count.load(Ordering::Acquire) as usize;
        for i in 0..count {
            if let Some(dev) = self.devices[i] {
                if let Some(slot) = dev.alloc_slot() {
                    return Ok((i, slot));
                }
            }
        }
        Err(SwapError::NoSlot)
    }

    /// Libère un slot sur le dispositif `dev_idx`.
    pub fn free_slot(&self, dev_idx: usize, slot: SwapSlot) {
        if dev_idx < MAX_SWAP_DEVICES {
            if let Some(dev) = self.devices[dev_idx] {
                dev.free_slot(slot);
                self.stats_slots_freed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Lit une page depuis le swap.
    ///
    /// # Safety
    /// `dst` doit être un frame physique valide.
    pub unsafe fn read_page(
        &self,
        dev_idx: usize,
        slot: SwapSlot,
        dst: PhysAddr,
    ) -> Result<(), SwapError> {
        if dev_idx >= MAX_SWAP_DEVICES {
            return Err(SwapError::InvalidSlot);
        }
        let dev = self.devices[dev_idx].ok_or(SwapError::NoDevice)?;
        let res = dev.read_page(slot, dst);
        match &res {
            Ok(_) => {
                self.stats_reads.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.stats_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        res
    }

    /// Écrit une page vers le swap.
    ///
    /// # Safety
    /// `src` doit être un frame physique valide.
    pub unsafe fn write_page(
        &self,
        dev_idx: usize,
        slot: SwapSlot,
        src: PhysAddr,
    ) -> Result<(), SwapError> {
        if dev_idx >= MAX_SWAP_DEVICES {
            return Err(SwapError::InvalidSlot);
        }
        let dev = self.devices[dev_idx].ok_or(SwapError::NoDevice)?;
        let res = dev.write_page(slot, src);
        match &res {
            Ok(_) => {
                self.stats_writes.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.stats_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        res
    }

    /// Capacité totale en pages (somme de tous les dispositifs).
    pub fn total_capacity(&self) -> u64 {
        let count = self.count.load(Ordering::Acquire) as usize;
        (0..count)
            .filter_map(|i| self.devices[i])
            .map(|d| d.capacity_pages())
            .sum()
    }

    /// Pages libres totales.
    pub fn total_free(&self) -> u64 {
        let count = self.count.load(Ordering::Acquire) as usize;
        (0..count)
            .filter_map(|i| self.devices[i])
            .map(|d| d.free_pages())
            .sum()
    }
}

pub static SWAP_BACKEND: SwapBackendRegistry = SwapBackendRegistry::new();

// ─────────────────────────────────────────────────────────────────────────────
// ENTRÉE SWAP (dans la table des pages)
// ─────────────────────────────────────────────────────────────────────────────

/// Encodage d'une entrée swap dans le PTE (non présent avec info swap).
/// Format : bits [63:12] = slot, bits [11:9] = dev_idx, bit [0] = 0 (non présent).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct SwapPte(pub u64);

impl SwapPte {
    pub const SWAP_FLAG: u64 = 0b10; // bit 1 = swap marker (bit 0 = 0 = not present)

    pub fn encode(dev_idx: usize, slot: SwapSlot) -> Self {
        let s = (slot.as_u64() << 12) | ((dev_idx as u64 & 0x7) << 1) | Self::SWAP_FLAG;
        SwapPte(s)
    }

    pub fn is_swap(raw: u64) -> bool {
        raw & 1 == 0 && raw & Self::SWAP_FLAG != 0
    }

    pub fn dev_idx(self) -> usize {
        ((self.0 >> 1) & 0x7) as usize
    }
    pub fn slot(self) -> SwapSlot {
        SwapSlot(self.0 >> 12)
    }
    pub fn raw(self) -> u64 {
        self.0
    }
}
