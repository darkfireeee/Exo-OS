// kernel/src/memory/dma/core/types.rs
//
// Types fondamentaux DMA — partagés par tous les sous-modules DMA.
// COUCHE 0 — zéro dépendance externe.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::memory::core::constants::PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// IDENTIFIANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'un canal DMA (index global dans la table des canaux).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct DmaChannelId(pub u32);

/// Identifiant d'un domaine IOMMU.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct IommuDomainId(pub u32);

/// Identifiant d'une transaction DMA en cours.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct DmaTransactionId(pub u64);

impl DmaTransactionId {
    pub const INVALID: Self = DmaTransactionId(0);

    /// Génère un ID unique via compteur atomique global.
    pub fn generate() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        DmaTransactionId(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ADRESSE DMA (IOVA)
// ─────────────────────────────────────────────────────────────────────────────

/// Adresse I/O virtuelle (IOVA) vue par le périphérique après translation IOMMU.
/// À distinguer de `PhysAddr` (vue CPU).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(transparent)]
pub struct IovaAddr(pub u64);

impl IovaAddr {
    #[inline] pub const fn new(v: u64) -> Self { IovaAddr(v) }
    #[inline] pub const fn as_u64(self) -> u64 { self.0 }
    #[inline] pub const fn is_aligned(self, align: u64) -> bool {
        self.0 & (align - 1) == 0
    }
    #[inline] pub fn page_aligned(self) -> Self {
        IovaAddr(self.0 & !(PAGE_SIZE as u64 - 1))
    }
    #[inline] pub const fn zero() -> Self { IovaAddr(0) }
    #[inline] pub const fn is_zero(self) -> bool { self.0 == 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// DIRECTION DE TRANSFERT
// ─────────────────────────────────────────────────────────────────────────────

/// Direction d'un transfert DMA.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum DmaDirection {
    /// Device → RAM (lecture par le CPU, écriture par le device).
    ToDevice   = 0,
    /// RAM → Device (lecture par le device, écriture CPU).
    FromDevice = 1,
    /// Bidirectionnel.
    Bidirection = 2,
    /// Aucun mouvement (memset, test).
    None = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// FLAGS DE MAPPING DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Flags modifiant le comportement d'un mapping DMA.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct DmaMapFlags(pub u32);

impl DmaMapFlags {
    pub const NONE:        Self = DmaMapFlags(0);
    /// Le mapping doit être dans les 16 premiers MiB (DMA ISA legacy).
    pub const DMA16:       Self = DmaMapFlags(1 << 0);
    /// Le mapping doit rester sous 4 GiB (DMA32).
    pub const DMA32:       Self = DmaMapFlags(1 << 1);
    /// Le buffer est contigu physiquement (pas de scatter-gather).
    pub const CONTIGUOUS:  Self = DmaMapFlags(1 << 2);
    /// Cache cohérence forcée (flush/invalidate autour des transferts).
    pub const CACHE_SYNC:  Self = DmaMapFlags(1 << 3);
    /// Ne pas insérer dans la table IOMMU (raw passthrough).
    pub const BYPASS_IOMMU: Self = DmaMapFlags(1 << 4);
    /// Buffer permanent (ne pas ré-allouer à chaque transfert).
    pub const PERSISTENT:  Self = DmaMapFlags(1 << 5);
    /// Lecture seule pour le device.
    pub const READ_ONLY:   Self = DmaMapFlags(1 << 6);
    /// Écriture seule pour le device.
    pub const WRITE_ONLY:  Self = DmaMapFlags(1 << 7);
    /// Invalide le cache après transfert → Device (for_cpu flush).
    pub const SYNC_FOR_CPU: Self = DmaMapFlags(1 << 8);
    /// Flush le cache avant transfert → Device (for_device flush).
    pub const SYNC_FOR_DEV: Self = DmaMapFlags(1 << 9);

    #[inline] pub fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
    #[inline] pub fn set(self, other: Self) -> Self { DmaMapFlags(self.0 | other.0) }
    #[inline] pub fn clear(self, other: Self) -> Self { DmaMapFlags(self.0 & !other.0) }
}

impl core::ops::BitOr for DmaMapFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { DmaMapFlags(self.0 | rhs.0) }
}

// ─────────────────────────────────────────────────────────────────────────────
// ÉTAT D'UNE TRANSACTION
// ─────────────────────────────────────────────────────────────────────────────

/// État d'une transaction DMA.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum DmaTransactionState {
    /// Slot libre.
    Free      = 0,
    /// Transaction créée mais pas encore soumise.
    Pending   = 1,
    /// Transaction soumise au contrôleur.
    Submitted = 2,
    /// Transfert en cours d'exécution.
    Running   = 3,
    /// Transfert terminé avec succès.
    Done      = 4,
    /// Transfert terminé avec une erreur.
    Error     = 5,
    /// Transaction annulée.
    Cancelled = 6,
}

// ─────────────────────────────────────────────────────────────────────────────
// PRIORITÉ DE CANAL
// ─────────────────────────────────────────────────────────────────────────────

/// Priorité d'un canal DMA (pour l'ordonnancement des requêtes).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
#[repr(u8)]
pub enum DmaPriority {
    Low      = 0,
    Normal   = 1,
    High     = 2,
    Realtime = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// CAPACITÉS D'UN CANAL
// ─────────────────────────────────────────────────────────────────────────────

/// Capacités déclarées par un canal DMA (bitfield).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct DmaCapabilities(pub u32);

impl DmaCapabilities {
    pub const MEMCPY:          Self = DmaCapabilities(1 << 0);
    pub const MEMSET:          Self = DmaCapabilities(1 << 1);
    pub const XOR:             Self = DmaCapabilities(1 << 2);
    pub const PQ:              Self = DmaCapabilities(1 << 3);
    pub const INTERRUPT:       Self = DmaCapabilities(1 << 4);
    pub const SCATTER_GATHER:  Self = DmaCapabilities(1 << 5);
    pub const CYCLIC:          Self = DmaCapabilities(1 << 6);
    pub const INTERLEAVED:     Self = DmaCapabilities(1 << 7);
    pub const SLAVE_SG:        Self = DmaCapabilities(1 << 8);
    pub const PRIVATE:         Self = DmaCapabilities(1 << 9);
    pub const ASYNC_TX:        Self = DmaCapabilities(1 << 10);
    pub const REPEAT:          Self = DmaCapabilities(1 << 11);
    pub const LOAD_EOT:        Self = DmaCapabilities(1 << 12);
    pub const NONE:            Self = DmaCapabilities(0);

    #[inline] pub fn has(self, cap: Self) -> bool { self.0 & cap.0 == cap.0 }
    #[inline] pub fn set(self, cap: Self) -> Self { DmaCapabilities(self.0 | cap.0) }
}

// ─────────────────────────────────────────────────────────────────────────────
// ERREURS DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs spécifiques au sous-système DMA.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum DmaError {
    /// Pas de canal DMA disponible.
    NoChannel       = 0,
    /// Mémoire DMA insuffisante.
    OutOfMemory     = 1,
    /// Paramètres invalides (taille, alignement…).
    InvalidParams   = 2,
    /// Timeout lors d'un transfert.
    Timeout         = 3,
    /// Erreur matérielle (bus, parity…).
    HardwareError   = 4,
    /// Erreur IOMMU (mapping refusé, faute de page).
    IommuFault      = 5,
    /// Canal non initialisé.
    NotInitialized  = 6,
    /// Transaction déjà soumise.
    AlreadySubmitted = 7,
    /// Transaction annulée.
    Cancelled       = 8,
    /// Buffer non aligné sur la granularité requise.
    MisalignedBuffer = 9,
    /// Adresse non dans la zone DMA requise.
    WrongZone       = 10,
    /// Opération non supportée par ce canal.
    NotSupported    = 11,
}
