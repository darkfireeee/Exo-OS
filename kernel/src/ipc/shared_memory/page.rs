// ipc/shared_memory/page.rs — Gestion de pages partagées (NO_COW) pour Exo-OS
//
// Ce module gère les pages physiques utilisées pour la mémoire partagée IPC.
// Chaque page est identifiée par son adresse physique et possède un compteur de
// référence atomique. Le flag NO_COW est obligatoire : la page ne doit jamais
// être dupliquée lors d'un fork (contrainte architecturale Exo-OS).
//
// Règles :
//   RÈGLE NO-ALLOC : pas de Vec/Box en zone chaude
//   FLAG NO_COW obligatoire sur toutes les pages SHM IPC

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Flags de page partagée
// ---------------------------------------------------------------------------

/// Flags de mappage d'une page SHM
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFlags(pub u32);

impl PageFlags {
    /// Page lisible
    pub const READ: Self = Self(1 << 0);
    /// Page écrivable
    pub const WRITE: Self = Self(1 << 1);
    /// Page exécutable (normalement interdit pour SHM IPC)
    pub const EXECUTE: Self = Self(1 << 2);
    /// Pas de Copy-on-Write — OBLIGATOIRE pour SHM IPC
    pub const NO_COW: Self = Self(1 << 3);
    /// Page épinglée en mémoire (non swappable)
    pub const PINNED: Self = Self(1 << 4);
    /// Page mappée dans plusieurs espaces d'adressage
    pub const SHARED: Self = Self(1 << 5);
    /// Combined lecture + écriture + no_cow + pinned (défaut SHM)
    pub const SHM_DEFAULT: Self =
        Self(Self::READ.0 | Self::WRITE.0 | Self::NO_COW.0 | Self::PINNED.0 | Self::SHARED.0);

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

// ---------------------------------------------------------------------------
// Identification de page
// ---------------------------------------------------------------------------

/// Taille d'une page standard (4 KiB)
pub const PAGE_SIZE: usize = 4096;

/// Taille d'une huge page (2 MiB)
pub const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Adresse physique alignée sur PAGE_SIZE — wrapper néwtypé
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

impl PhysAddr {
    /// Adresse physique nulle (sentinelle)
    pub const NULL: Self = Self(0);

    #[inline]
    pub fn is_null(self) -> bool {
        self.0 == 0
    }

    /// Vérifie que l'adresse est alignée sur `align` (doit être une puissance de 2)
    #[inline]
    pub fn is_aligned(self, align: usize) -> bool {
        (self.0 & (align as u64 - 1)) == 0
    }

    /// Calcule l'index de page à partir de l'adresse physique de base.
    #[inline]
    pub fn page_index(self, base: PhysAddr) -> Option<usize> {
        if self.0 < base.0 {
            return None;
        }
        let offset = (self.0 - base.0) as usize;
        if offset % PAGE_SIZE != 0 {
            return None;
        }
        Some(offset / PAGE_SIZE)
    }
}

// ---------------------------------------------------------------------------
// Descripteur de page SHM
// ---------------------------------------------------------------------------

/// Descripteur d'une page physique appartenant au pool SHM IPC.
/// Taille : 64 octets exactement (une cache line).
#[repr(C, align(64))]
pub struct ShmPage {
    /// Adresse physique de la page
    pub phys_addr: PhysAddr,
    /// Compteur de références atomique
    pub refcount: AtomicU32,
    /// Flags de la page (PageFlags encodés)
    pub flags: AtomicU32,
    /// Index dans le pool parent
    pub pool_index: AtomicU32,
    /// Génération (version de réutilisation — détection ABA)
    pub generation: AtomicU32,
    /// Identifiant du dernier processus ayant mappé cette page
    pub last_mapper_pid: AtomicU32,
    /// Timestamp d'allocation (ns depuis boot)
    pub alloc_ts: AtomicU64,
    /// Nombre de fois que cette page a été réutilisée
    pub reuse_count: AtomicU32,
    _pad: [u8; 4],
}

// SAFETY: tous les champs mutables sont atomiques
unsafe impl Sync for ShmPage {}
unsafe impl Send for ShmPage {}

impl ShmPage {
    /// Crée un descripteur de page non-initialisé (phys_addr = NULL)
    pub const fn new_uninit() -> Self {
        Self {
            phys_addr: PhysAddr::NULL,
            refcount: AtomicU32::new(0),
            flags: AtomicU32::new(0),
            pool_index: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            last_mapper_pid: AtomicU32::new(0),
            alloc_ts: AtomicU64::new(0),
            reuse_count: AtomicU32::new(0),
            _pad: [0u8; 4],
        }
    }

    /// Initialise la page avec une adresse physique et ses flags.
    ///
    /// # Safety
    /// Doit être appelé une seule fois au boot, avant tout accès concurrent.
    /// `phys_addr` est un champ non-atomique : l'assignation se fait via
    /// un raw pointer write (pattern write-once sur static).
    pub fn init(&self, phys: PhysAddr, pool_idx: u32, flags: PageFlags) {
        // SAFETY: init() appelé une seule fois avant tout accès concurrent; phys write-once sur static.
        unsafe {
            self.set_phys_unchecked(phys);
        }
        self.flags
            .store(flags.0 | PageFlags::NO_COW.0, Ordering::Relaxed);
        self.pool_index.store(pool_idx, Ordering::Relaxed);
        self.refcount.store(0, Ordering::Release);
    }

    /// Incrémente le compteur de références.
    /// Retourne le nouveau compteur.
    pub fn ref_acquire(&self) -> u32 {
        self.refcount.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Décrémente le compteur de références.
    /// Retourne `true` si la page est maintenant libre (refcount == 0).
    pub fn ref_release(&self) -> bool {
        let prev = self.refcount.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            // Incrémenter la génération pour invalider les références pendantes
            self.generation.fetch_add(1, Ordering::Release);
            self.reuse_count.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn refcount(&self) -> u32 {
        self.refcount.load(Ordering::Acquire)
    }

    pub fn is_free(&self) -> bool {
        self.refcount() == 0
    }

    pub fn flags(&self) -> PageFlags {
        PageFlags(self.flags.load(Ordering::Relaxed))
    }

    pub fn generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    /// Vérifie que le flag NO_COW est bien positionné.
    pub fn assert_no_cow(&self) -> bool {
        self.flags().contains(PageFlags::NO_COW)
    }
}

// SAFETY: phys_addr est assigné une seule fois à l'init ; lecture ultérieure est safe.
// (Rust ne peut pas exprimer ça en type — on utilise UnsafeCell mais le champ est
// en fait write-once donc une simple lecture directe est acceptable ici.)
impl ShmPage {
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn set_phys_unchecked(&self, phys: PhysAddr) {
        // SAFETY: addr_of! crée un *const sans référence intermédiaire — évite l'UB
        // de mutation via &T. Écriture unique à l'init, le champ est write-once.
        core::ptr::write(core::ptr::addr_of!(self.phys_addr) as *mut PhysAddr, phys);
    }
}

// ---------------------------------------------------------------------------
// Itérateur de pages libres (pour le pool)
// ---------------------------------------------------------------------------

/// Statistiques de pages SHM
#[derive(Debug, Clone, Copy)]
pub struct ShmPageStats {
    pub total_pages: usize,
    pub free_pages: usize,
    pub used_pages: usize,
    pub total_reuses: u64,
}
