// kernel/src/memory/huge_pages/hugetlbfs.rs
//
// Hugetlbfs — pool de huge pages 1 GiB pré-réservées.
// Couche 0 — aucune dépendance externe sauf `spin`.
//
// Principe :
//   Un pool statique de HUGETLB_MAX_PAGES huge pages (configurable) est
//   réservé au boot. Les allocations viennent de ce pool — contrairement
//   aux THP qui utilisent le buddy dynamiquement.
//   Cela garantit la disponibilité des huge pages même sous pression mémoire.
//
// Tailles supportées :
//   2 MiB (ordre 9)  — interopérable avec thp.rs
//   1 GiB (ordre 18) — pages gigantesques (processeurs récents)

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::{AllocFlags, Frame, PhysAddr, PAGE_SIZE};
use crate::memory::physical::allocator::buddy::{alloc_pages, free_pages};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

/// Ordre buddy pour une huge page 1 GiB (1 GiB / 4 KiB = 262144 = 2^18).
pub const GIGA_PAGE_ORDER: u32 = 18;

/// Capacité maximale du pool hugetlb statique.
pub const HUGETLB_MAX_POOL: usize = 512;

// ─────────────────────────────────────────────────────────────────────────────
// SLOT DU POOL
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'une huge page dans le pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HugeTlbSize {
    /// Page de 2 MiB (ordre 9).
    TwoMiB = 9,
    /// Page de 1 GiB (ordre 18).
    OneGiB = 18,
}

impl HugeTlbSize {
    #[inline]
    pub fn order(self) -> usize {
        self as usize
    }
    #[inline]
    pub fn size_bytes(self) -> usize {
        PAGE_SIZE << (self as usize)
    }
}

/// Slot dans le pool hugetlb.
#[derive(Clone, Copy)]
struct HugeTlbSlot {
    /// Adresse physique du frame (0 si libre).
    phys: u64,
    /// Taille (ordre buddy).
    order: u8,
    /// Slot occupé (true = alloué à un appelant).
    occupied: bool,
}

impl HugeTlbSlot {
    const EMPTY: HugeTlbSlot = HugeTlbSlot {
        phys: 0,
        order: 0,
        occupied: false,
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES HUGETLB
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques du pool hugetlb.
pub struct HugeTlbStats {
    /// Nombre de slots dans le pool (taille configurable au boot).
    pub pool_size: AtomicU32,
    /// Nombre de slots actuellement alloués.
    pub in_use: AtomicU32,
    /// Nombre total d'allocations réussies.
    pub alloc_success: AtomicU64,
    /// Nombre d'allocations échouées (pool épuisé).
    pub alloc_fail: AtomicU64,
    /// Nombre total de libérations.
    pub frees: AtomicU64,
}

impl HugeTlbStats {
    const fn new() -> Self {
        HugeTlbStats {
            pool_size: AtomicU32::new(0),
            in_use: AtomicU32::new(0),
            alloc_success: AtomicU64::new(0),
            alloc_fail: AtomicU64::new(0),
            frees: AtomicU64::new(0),
        }
    }
}

pub static HUGETLB_STATS: HugeTlbStats = HugeTlbStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// POOL HUGETLB
// ─────────────────────────────────────────────────────────────────────────────

struct HugeTlbPoolInner {
    slots: [HugeTlbSlot; HUGETLB_MAX_POOL],
    count: usize, // nombre de slots initialisés
}

impl HugeTlbPoolInner {
    const fn new() -> Self {
        HugeTlbPoolInner {
            slots: [HugeTlbSlot::EMPTY; HUGETLB_MAX_POOL],
            count: 0,
        }
    }
}

/// Pool hugetlb global.
pub struct HugeTlbPool {
    inner: Mutex<HugeTlbPoolInner>,
    initialized: AtomicBool,
}

impl HugeTlbPool {
    const fn new() -> Self {
        HugeTlbPool {
            inner: Mutex::new(HugeTlbPoolInner::new()),
            initialized: AtomicBool::new(false),
        }
    }

    /// Initialise le pool en pré-allouant `n_2mib` pages 2 MiB et `n_1gib` pages 1 GiB.
    ///
    /// # Safety : CPL 0, appelé une seule fois au boot.
    pub unsafe fn init(&self, n_2mib: usize, n_1gib: usize) {
        if self.initialized.swap(true, Ordering::AcqRel) {
            return;
        }
        let total = (n_2mib + n_1gib).min(HUGETLB_MAX_POOL);
        let mut inner = self.inner.lock();

        // Pré-allouer les pages 2 MiB.
        let alloc_2mib = n_2mib.min(total);
        for _ in 0..alloc_2mib {
            if inner.count >= HUGETLB_MAX_POOL {
                break;
            }
            match alloc_pages(HugeTlbSize::TwoMiB.order(), AllocFlags::MOVABLE) {
                Ok(frame) => {
                    let idx = inner.count;
                    inner.slots[idx] = HugeTlbSlot {
                        phys: frame.start_address().as_u64(),
                        order: HugeTlbSize::TwoMiB as u8,
                        occupied: false,
                    };
                    inner.count += 1;
                }
                Err(_) => break, // mémoire insuffisante — on s'arrête
            }
        }

        // Pré-allouer les pages 1 GiB.
        let alloc_1gib = n_1gib.min(HUGETLB_MAX_POOL - inner.count);
        for _ in 0..alloc_1gib {
            if inner.count >= HUGETLB_MAX_POOL {
                break;
            }
            match alloc_pages(HugeTlbSize::OneGiB.order(), AllocFlags::NONE) {
                Ok(frame) => {
                    let idx = inner.count;
                    inner.slots[idx] = HugeTlbSlot {
                        phys: frame.start_address().as_u64(),
                        order: HugeTlbSize::OneGiB as u8,
                        occupied: false,
                    };
                    inner.count += 1;
                }
                Err(_) => break,
            }
        }

        HUGETLB_STATS
            .pool_size
            .store(inner.count as u32, Ordering::Relaxed);
    }

    /// Alloue une huge page du pool.
    ///
    /// Préfère les pages libres correspondant à `size`. Si le pool est épuisé
    /// pour la taille demandée, retourne `None` (jamais de fallback buddy depuis
    /// hugetlb — c'est la responsabilité de l'appelant).
    pub fn alloc(&self, size: HugeTlbSize) -> Option<Frame> {
        if !self.initialized.load(Ordering::Acquire) {
            return None;
        }
        let mut inner = self.inner.lock();
        let target_order = size as u8;
        let count = inner.count;

        for slot in inner.slots[..count].iter_mut() {
            if slot.order == target_order && !slot.occupied && slot.phys != 0 {
                slot.occupied = true;
                let frame = Frame::from_phys_addr(PhysAddr::new(slot.phys));
                drop(inner);
                HUGETLB_STATS.in_use.fetch_add(1, Ordering::Relaxed);
                HUGETLB_STATS.alloc_success.fetch_add(1, Ordering::Relaxed);
                return Some(frame);
            }
        }

        HUGETLB_STATS.alloc_fail.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Libère une huge page vers le pool.
    ///
    /// # Safety : `frame` doit avoir été alloué via `HugeTlbPool::alloc`.
    pub unsafe fn free(&self, frame: Frame, size: HugeTlbSize) {
        let phys = frame.start_address().as_u64();
        let mut inner = self.inner.lock();
        let count = inner.count;

        for slot in inner.slots[..count].iter_mut() {
            if slot.phys == phys && slot.order == size as u8 && slot.occupied {
                slot.occupied = false;
                drop(inner);
                HUGETLB_STATS.in_use.fetch_sub(1, Ordering::Relaxed);
                HUGETLB_STATS.frees.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // Frame non trouvé dans le pool — libérer dans le buddy (ne devrait pas arriver).
        let _ = free_pages(frame, size.order());
        HUGETLB_STATS.frees.fetch_add(1, Ordering::Relaxed);
    }

    /// Retourne le nombre de slots libres pour la taille donnée.
    pub fn free_count(&self, size: HugeTlbSize) -> usize {
        if !self.initialized.load(Ordering::Acquire) {
            return 0;
        }
        let inner = self.inner.lock();
        inner.slots[..inner.count]
            .iter()
            .filter(|s| s.order == size as u8 && !s.occupied && s.phys != 0)
            .count()
    }

    /// Ajuste dynamiquement la taille du pool.
    /// Alloue ou libère des slots pour atteindre `target_count` pour `size`.
    ///
    /// # Safety : CPL 0.
    pub unsafe fn resize(&self, size: HugeTlbSize, target_count: usize) {
        let current_free = self.free_count(size);
        if current_free == target_count {
            return;
        }

        if current_free < target_count {
            // Agrandir : allouer des pages supplémentaires.
            let to_alloc = target_count - current_free;
            let mut inner = self.inner.lock();
            for _ in 0..to_alloc {
                if inner.count >= HUGETLB_MAX_POOL {
                    break;
                }
                match alloc_pages(size.order(), AllocFlags::MOVABLE) {
                    Ok(frame) => {
                        let idx = inner.count;
                        inner.slots[idx] = HugeTlbSlot {
                            phys: frame.start_address().as_u64(),
                            order: size as u8,
                            occupied: false,
                        };
                        inner.count += 1;
                        HUGETLB_STATS.pool_size.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => break,
                }
            }
        } else {
            // Réduire : libérer des pages inutilisées.
            let to_free = current_free - target_count;
            let mut freed = 0usize;
            let mut inner = self.inner.lock();
            let count = inner.count;
            for slot in inner.slots[..count].iter_mut() {
                if freed >= to_free {
                    break;
                }
                if slot.order == size as u8 && !slot.occupied && slot.phys != 0 {
                    let frame = Frame::from_phys_addr(PhysAddr::new(slot.phys));
                    let _ = free_pages(frame, size.order());
                    slot.phys = 0;
                    slot.occupied = false;
                    slot.order = 0;
                    HUGETLB_STATS.pool_size.fetch_sub(1, Ordering::Relaxed);
                    freed += 1;
                }
            }
        }
    }
}

/// Instance globale du pool hugetlb.
pub static HUGETLB_POOL: HugeTlbPool = HugeTlbPool::new();

// ─────────────────────────────────────────────────────────────────────────────
// API PUBLIQUE
// ─────────────────────────────────────────────────────────────────────────────

/// Alloue une huge page 2 MiB depuis le pool hugetlb.
#[inline]
pub fn hugetlb_alloc_2mib() -> Option<Frame> {
    HUGETLB_POOL.alloc(HugeTlbSize::TwoMiB)
}

/// Alloue une huge page 1 GiB depuis le pool hugetlb.
#[inline]
pub fn hugetlb_alloc_1gib() -> Option<Frame> {
    HUGETLB_POOL.alloc(HugeTlbSize::OneGiB)
}

/// Libère une huge page 2 MiB vers le pool.
///
/// # Safety : `frame` doit avoir été alloué via `hugetlb_alloc_2mib`.
#[inline]
pub unsafe fn hugetlb_free_2mib(frame: Frame) {
    HUGETLB_POOL.free(frame, HugeTlbSize::TwoMiB);
}

/// Libère une huge page 1 GiB vers le pool.
///
/// # Safety : `frame` doit avoir été alloué via `hugetlb_alloc_1gib`.
#[inline]
pub unsafe fn hugetlb_free_1gib(frame: Frame) {
    HUGETLB_POOL.free(frame, HugeTlbSize::OneGiB);
}

/// Retourne le nombre de huge pages 2 MiB disponibles dans le pool.
#[inline]
pub fn hugetlb_free_2mib_count() -> usize {
    HUGETLB_POOL.free_count(HugeTlbSize::TwoMiB)
}

/// Retourne le nombre de huge pages 1 GiB disponibles dans le pool.
#[inline]
pub fn hugetlb_free_1gib_count() -> usize {
    HUGETLB_POOL.free_count(HugeTlbSize::OneGiB)
}

/// Initialise le pool hugetlb au boot.
///
/// # Safety : CPL 0.
pub unsafe fn init(n_2mib: usize, n_1gib: usize) {
    HUGETLB_POOL.init(n_2mib, n_1gib);
}
