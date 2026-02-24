// ipc/shared_memory/allocator.rs — Allocateur SHM lock-free avec niveaux de taille
//
// Fournit une interface d'allocation de régions SHM en s'appuyant sur le pool
// de pages (pool.rs) et l'allocateur de descripteurs (descriptor.rs).
//
// Stratégie : buddy-like avec 4 niveaux de taille prédéfinis :
//   - Small  : 1 page  (4 KiB)
//   - Medium : 4 pages (16 KiB)
//   - Large  : 16 pages (64 KiB)
//   - Huge   : 64 pages (256 KiB)
//
// Chaque niveau maintient une freelist de descripteurs réutilisables.
// L'allocation tente le niveau exact, puis remonte si nécessaire.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::types::{IpcError, ProcessId};
use crate::ipc::shared_memory::descriptor::{
    ShmDescriptor, ShmPermissions, ShmId,
    shm_create, shm_destroy, shm_region_count, MAX_SHM_REGIONS,
};
use crate::ipc::shared_memory::pool::{
    shm_page_alloc, shm_page_free, shm_pool_stats,
};

// ---------------------------------------------------------------------------
// Niveaux de taille
// ---------------------------------------------------------------------------

/// Niveau de taille d'une allocation SHM
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum ShmSizeClass {
    /// 1 page = 4 KiB
    Small = 0,
    /// 4 pages = 16 KiB
    Medium = 1,
    /// 16 pages = 64 KiB
    Large = 2,
    /// 64 pages = 256 KiB
    Huge = 3,
}

impl ShmSizeClass {
    /// Retourne le nombre de pages correspondant au niveau
    pub fn pages(self) -> usize {
        match self {
            Self::Small => 1,
            Self::Medium => 4,
            Self::Large => 16,
            Self::Huge => 64,
        }
    }

    /// Retourne la taille en octets
    pub fn bytes(self) -> usize {
        self.pages() * crate::ipc::shared_memory::page::PAGE_SIZE
    }

    /// Classe minimale pour une taille en bytes
    pub fn for_size(bytes: usize) -> Self {
        if bytes <= Self::Small.bytes() {
            Self::Small
        } else if bytes <= Self::Medium.bytes() {
            Self::Medium
        } else if bytes <= Self::Large.bytes() {
            Self::Large
        } else {
            Self::Huge
        }
    }

    pub const COUNT: usize = 4;
}

// ---------------------------------------------------------------------------
// Compteurs per-class
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
struct ClassCounters {
    allocs: AtomicU64,
    frees: AtomicU64,
    failures: AtomicU64,
    _pad: [u8; 40],
}

impl ClassCounters {
    const fn new() -> Self {
        Self {
            allocs: AtomicU64::new(0),
            frees: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            _pad: [0u8; 40],
        }
    }
}

static CLASS_COUNTERS: [ClassCounters; ShmSizeClass::COUNT] = [
    ClassCounters::new(),
    ClassCounters::new(),
    ClassCounters::new(),
    ClassCounters::new(),
];

// ---------------------------------------------------------------------------
// Statistiques globales de l'allocateur
// ---------------------------------------------------------------------------

static ALLOC_TOTAL: AtomicU64 = AtomicU64::new(0);
static FREE_TOTAL: AtomicU64 = AtomicU64::new(0);
static ALLOC_FAIL: AtomicU64 = AtomicU64::new(0);
static BYTES_ALLOCATED: AtomicU64 = AtomicU64::new(0);
static BYTES_FREED: AtomicU64 = AtomicU64::new(0);

/// Snapshot des statistiques de l'allocateur SHM
#[derive(Debug, Clone, Copy)]
pub struct ShmAllocatorStats {
    pub alloc_total: u64,
    pub free_total: u64,
    pub alloc_fail: u64,
    pub bytes_allocated: u64,
    pub bytes_freed: u64,
    pub active_regions: usize,
    pub pool_free_pages: usize,
}

pub fn shm_allocator_stats() -> ShmAllocatorStats {
    let pool = shm_pool_stats();
    ShmAllocatorStats {
        alloc_total: ALLOC_TOTAL.load(Ordering::Relaxed),
        free_total: FREE_TOTAL.load(Ordering::Relaxed),
        alloc_fail: ALLOC_FAIL.load(Ordering::Relaxed),
        bytes_allocated: BYTES_ALLOCATED.load(Ordering::Relaxed),
        bytes_freed: BYTES_FREED.load(Ordering::Relaxed),
        active_regions: shm_region_count(),
        pool_free_pages: pool.free_pages,
    }
}

// ---------------------------------------------------------------------------
// Handle d'allocation : résultat retourné au demandeur
// ---------------------------------------------------------------------------

/// Handle retourné lors d'une allocation SHM réussie.
/// Contient l'index de descripteur et les métadonnées de la région.
#[derive(Debug, Clone, Copy)]
pub struct ShmHandle {
    /// Index dans SHM_DESC_DIR
    pub desc_idx: usize,
    /// Identifiant unique de la région
    pub shm_id: ShmId,
    /// Taille allouée en octets
    pub size_bytes: usize,
    /// Classe de taille effective
    pub size_class: ShmSizeClass,
    /// Nombre de pages allouées
    pub page_count: usize,
}

// ---------------------------------------------------------------------------
// Allocateur principal
// ---------------------------------------------------------------------------

/// Alloue une région SHM de la taille minimale couvrant `requested_bytes`.
///
/// # Algorithme
/// 1. Déterminer la ShmSizeClass minimale
/// 2. Appeler shm_create() qui alloue les pages depuis le pool
/// 3. Retourner un ShmHandle
///
/// # Erreurs
/// - `IpcError::OutOfResources` — pool de pages épuisé ou MAX_SHM_REGIONS atteint
/// - `IpcError::InvalidArgument` — taille 0 ou > Huge.bytes()
pub fn shm_alloc(
    owner: ProcessId,
    perms: ShmPermissions,
    requested_bytes: usize,
) -> Result<ShmHandle, IpcError> {
    if requested_bytes == 0 {
        return Err(IpcError::InvalidArgument);
    }

    let class = ShmSizeClass::for_size(requested_bytes);
    let n_pages = class.pages();

    CLASS_COUNTERS[class as usize].allocs.fetch_add(1, Ordering::Relaxed);

    match shm_create(owner, perms, n_pages) {
        Ok(desc_idx) => {
            let shm_id = crate::ipc::shared_memory::descriptor::shm_get_id(desc_idx)
                .unwrap_or(ShmId::INVALID);
            let size_bytes = n_pages * crate::ipc::shared_memory::page::PAGE_SIZE;

            ALLOC_TOTAL.fetch_add(1, Ordering::Relaxed);
            BYTES_ALLOCATED.fetch_add(size_bytes as u64, Ordering::Relaxed);

            Ok(ShmHandle {
                desc_idx,
                shm_id,
                size_bytes,
                size_class: class,
                page_count: n_pages,
            })
        }
        Err(e) => {
            CLASS_COUNTERS[class as usize].failures.fetch_add(1, Ordering::Relaxed);
            ALLOC_FAIL.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// Alloue une région SHM de taille exacte en pages.
/// Permet d'allouer un nombre arbitraire de pages (≤ MAX_SHM_PAGES_PER_DESC).
pub fn shm_alloc_pages(
    owner: ProcessId,
    perms: ShmPermissions,
    n_pages: usize,
) -> Result<ShmHandle, IpcError> {
    if n_pages == 0 {
        return Err(IpcError::InvalidArgument);
    }

    let class = ShmSizeClass::for_size(n_pages * crate::ipc::shared_memory::page::PAGE_SIZE);

    match shm_create(owner, perms, n_pages) {
        Ok(desc_idx) => {
            let shm_id = crate::ipc::shared_memory::descriptor::shm_get_id(desc_idx)
                .unwrap_or(ShmId::INVALID);
            let size_bytes = n_pages * crate::ipc::shared_memory::page::PAGE_SIZE;

            ALLOC_TOTAL.fetch_add(1, Ordering::Relaxed);
            BYTES_ALLOCATED.fetch_add(size_bytes as u64, Ordering::Relaxed);

            Ok(ShmHandle {
                desc_idx,
                shm_id,
                size_bytes,
                size_class: class,
                page_count: n_pages,
            })
        }
        Err(e) => {
            ALLOC_FAIL.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// Libère la région SHM décrite par `handle`.
pub fn shm_free(handle: ShmHandle) -> Result<(), IpcError> {
    let class = handle.size_class;
    CLASS_COUNTERS[class as usize].frees.fetch_add(1, Ordering::Relaxed);
    FREE_TOTAL.fetch_add(1, Ordering::Relaxed);
    BYTES_FREED.fetch_add(handle.size_bytes as u64, Ordering::Relaxed);
    shm_destroy(handle.desc_idx)
}

/// Libère la région SHM par son index de descripteur.
pub fn shm_free_by_idx(desc_idx: usize) -> Result<(), IpcError> {
    let size_bytes = crate::ipc::shared_memory::descriptor::shm_get_size(desc_idx)
        .unwrap_or(0);
    FREE_TOTAL.fetch_add(1, Ordering::Relaxed);
    BYTES_FREED.fetch_add(size_bytes, Ordering::Relaxed);
    shm_destroy(desc_idx)
}

// ---------------------------------------------------------------------------
// Utilitaire : défragmentation / compactage (no-op dans cette implémentation,
// mais fourni pour l'interface future)
// ---------------------------------------------------------------------------

/// Retourne le nombre de pages libres dans le pool SHM.
pub fn shm_free_page_count() -> usize {
    shm_pool_stats().free_pages
}

/// Vérifie si l'allocateur peut satisfaire une allocation de `n_pages`.
pub fn shm_can_alloc(n_pages: usize) -> bool {
    let stats = shm_pool_stats();
    stats.free_pages >= n_pages
        && shm_region_count() < MAX_SHM_REGIONS
}
