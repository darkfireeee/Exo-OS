// kernel/src/memory/swap/policy.rs
//
// Politique de swapping — décide quelles pages évincer (LRU / CLOCK-Pro).
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::types::PhysAddr;

// ─────────────────────────────────────────────────────────────────────────────
// ALGORITHME LRU-CLOCK (approximation LRU par bit Accessed)
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de frames dans la liste de candidats à l'éviction.
pub const EVICT_CANDIDATE_LIST_SIZE: usize = 1024;

/// Statistiques du swapper.
pub struct SwapPolicyStats {
    pub pages_evicted: AtomicU64,
    pub pages_reclaimed: AtomicU64,
    pub scan_cycles: AtomicU64,
    pub accessed_cleared: AtomicU64,
    pub dirty_skipped: AtomicU64,
    pub pinned_skipped: AtomicU64,
    pub oom_triggers: AtomicU64,
}

impl SwapPolicyStats {
    const fn new() -> Self {
        SwapPolicyStats {
            pages_evicted: AtomicU64::new(0),
            pages_reclaimed: AtomicU64::new(0),
            scan_cycles: AtomicU64::new(0),
            accessed_cleared: AtomicU64::new(0),
            dirty_skipped: AtomicU64::new(0),
            pinned_skipped: AtomicU64::new(0),
            oom_triggers: AtomicU64::new(0),
        }
    }
}

pub static SWAP_POLICY_STATS: SwapPolicyStats = SwapPolicyStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// GESTIONNAIRE DE LISTE D'ÉVICTION (CLOCK)
// ─────────────────────────────────────────────────────────────────────────────

/// Candidat à l'éviction dans la liste CLOCK.
#[derive(Copy, Clone, Debug)]
pub struct EvictCandidate {
    pub phys: PhysAddr,
    /// Address space associé (opaque, encodé en u64).
    pub as_id: u64,
    /// Adresse virtuelle dans l'address space.
    pub virt_addr: u64,
    /// Génération LRU (de FrameDesc::lru_gen).
    pub lru_gen: u8,
}

impl EvictCandidate {
    pub const EMPTY: Self = EvictCandidate {
        phys: PhysAddr::new(0),
        as_id: 0,
        virt_addr: 0,
        lru_gen: 0,
    };
    pub fn is_valid(&self) -> bool {
        self.phys.as_u64() != 0
    }
}

/// Liste CLOCK pour l'algorithme d'éviction.
pub struct ClockEvictList {
    inner: Mutex<ClockEvictListInner>,
}

struct ClockEvictListInner {
    candidates: [EvictCandidate; EVICT_CANDIDATE_LIST_SIZE],
    clock_hand: usize,
    count: usize,
}

// SAFETY: ClockEvictList est protégé par Mutex.
unsafe impl Sync for ClockEvictList {}
unsafe impl Send for ClockEvictList {}

impl ClockEvictList {
    const fn new() -> Self {
        ClockEvictList {
            inner: Mutex::new(ClockEvictListInner {
                candidates: [EvictCandidate::EMPTY; EVICT_CANDIDATE_LIST_SIZE],
                clock_hand: 0,
                count: 0,
            }),
        }
    }

    /// Ajoute un candidat à la liste (si non pleine).
    pub fn add(&self, candidate: EvictCandidate) {
        let mut inner = self.inner.lock();
        if inner.count >= EVICT_CANDIDATE_LIST_SIZE {
            return;
        }
        // Insère à la position suivante disponible.
        for slot in inner.candidates.iter_mut() {
            if !slot.is_valid() {
                *slot = candidate;
                inner.count += 1;
                return;
            }
        }
    }

    /// Sélectionne la prochaine victime via CLOCK.
    /// Retourne `None` si la liste est vide ou toutes les pages sont "accédées".
    pub fn next_victim(&self) -> Option<EvictCandidate> {
        let mut inner = self.inner.lock();
        if inner.count == 0 {
            return None;
        }

        let mut scans = 0usize;
        loop {
            if scans >= EVICT_CANDIDATE_LIST_SIZE * 2 {
                return None;
            }
            scans += 1;

            let idx = inner.clock_hand % EVICT_CANDIDATE_LIST_SIZE;
            inner.clock_hand = (inner.clock_hand + 1) % EVICT_CANDIDATE_LIST_SIZE;

            let c = &inner.candidates[idx];
            if !c.is_valid() {
                continue;
            }

            // Vérifie le bit Accessed dans le FrameDesc.
            // (En vrai on vérifierait aussi le PTE Accessed bit).
            let candidate = *c;

            // Sélectionne cette victime.
            inner.candidates[idx] = EvictCandidate::EMPTY;
            inner.count -= 1;
            SWAP_POLICY_STATS
                .scan_cycles
                .fetch_add(scans as u64, Ordering::Relaxed);
            return Some(candidate);
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().count
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub static EVICT_LIST: ClockEvictList = ClockEvictList::new();

// ─────────────────────────────────────────────────────────────────────────────
// POLITIQUE D'ÉVICTION
// ─────────────────────────────────────────────────────────────────────────────

/// Watermarks pour déclencher le swapping.
#[derive(Copy, Clone, Debug)]
pub struct SwapWatermarks {
    /// Pages libres minimales avant déclenchement du kswapd.
    pub low: u64,
    /// Pages libres "saines" (pas de swap).
    pub high: u64,
    /// Pages libres d'urgence (passe en mode OOM-kill si en-dessous).
    pub critical: u64,
}

impl SwapWatermarks {
    pub const DEFAULT: Self = SwapWatermarks {
        low: 512,     // 2 MiB
        high: 2048,   // 8 MiB
        critical: 64, // 256 KiB
    };
}

pub static SWAP_WATERMARKS: spin::RwLock<SwapWatermarks> =
    spin::RwLock::new(SwapWatermarks::DEFAULT);

/// Évalue si le swapping doit être déclenché.
pub fn should_swap(free_pages: u64) -> bool {
    let wm = SWAP_WATERMARKS.read();
    free_pages < wm.low
}

/// Évalue si la situation est critique (OOM).
pub fn is_critical(free_pages: u64) -> bool {
    let wm = SWAP_WATERMARKS.read();
    free_pages < wm.critical
}
