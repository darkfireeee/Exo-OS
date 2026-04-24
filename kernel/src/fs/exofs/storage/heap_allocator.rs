// kernel/src/fs/exofs/storage/heap_allocator.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Allocateur de heap ExoFS — gestion des extents physiques dans le heap
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// L'allocateur gère l'espace disque dans la zone heap [HEAP_START, heap_end).
// Il utilise une HeapFreeMap (bitmap) pour suivre les blocs libres et
// implémente un algorithme first-fit avec coalescence.
//
// Règles respectées :
// - ARITH-02 : checked_add/checked_mul pour TOUS les calculs d'offset.
// - OOM-02   : (non applicable ici, états internes dans HeapFreeMap).
// - LOCK-04  : SpinLock léger — pas d'I/O dedans.

use crate::fs::exofs::core::{DiskOffset, ExofsError, ExofsResult};
use crate::fs::exofs::storage::heap_coalesce::{CoalesceOptions, HeapCoalescer};
use crate::fs::exofs::storage::heap_free_map::HeapFreeMap;
use crate::fs::exofs::storage::layout::BLOCK_SIZE;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;
use crate::scheduler::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Extent — description d'un bloc alloué
// ─────────────────────────────────────────────────────────────────────────────

/// Extent alloué sur le disque.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Extent {
    /// Offset de début (en octets depuis le début du disque).
    pub offset: DiskOffset,
    /// Taille allouée en octets (multiple de BLOCK_SIZE).
    pub size: u64,
}

impl Extent {
    /// Offset de fin (exclusif).
    #[inline]
    pub fn end(&self) -> ExofsResult<DiskOffset> {
        self.offset
            .0
            .checked_add(self.size)
            .ok_or(ExofsError::OffsetOverflow)
            .map(DiskOffset)
    }

    /// Nombre de blocs de 4 KB dans cet extent.
    #[inline]
    pub fn block_count(&self) -> u64 {
        self.size / BLOCK_SIZE
    }

    /// Vérifie que cet extent est valide (size > 0, aligné sur BLOCK_SIZE).
    pub fn is_valid(&self) -> bool {
        self.size > 0 && self.size % BLOCK_SIZE == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AllocationPolicy — stratégie d'allocation
// ─────────────────────────────────────────────────────────────────────────────

/// Stratégie de recherche de blocs libres.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AllocationPolicy {
    /// First-fit depuis le début de la carte.
    FirstFit,
    /// First-fit depuis le hint (position de la dernière allocation).
    NextFit,
    /// Best-fit (run libre le plus proche de la taille demandée).
    BestFit,
}

impl Default for AllocationPolicy {
    fn default() -> Self {
        AllocationPolicy::NextFit
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HeapAllocatorState — état mutable sous SpinLock
// ─────────────────────────────────────────────────────────────────────────────

struct HeapAllocatorState {
    free_map: HeapFreeMap,
    /// Dernier bloc alloué (hint pour NextFit).
    last_alloc: u64,
    /// Compteur d'allocations depuis la dernière coalescence.
    allocs_since_coalesce: u64,
}

impl HeapAllocatorState {
    fn new(total_blocks: u64) -> Result<Self, ExofsError> {
        Ok(Self {
            free_map: HeapFreeMap::new(total_blocks)?,
            last_alloc: 0,
            allocs_since_coalesce: 0,
        })
    }

    /// Cherche `n_blocks` blocs libres contigus selon la politique.
    fn find_blocks(&mut self, n_blocks: u64, policy: AllocationPolicy) -> Option<u64> {
        match policy {
            AllocationPolicy::FirstFit => self.free_map.find_free_run(n_blocks),
            AllocationPolicy::NextFit => {
                self.free_map.find_free_run_hint(n_blocks, self.last_alloc)
            }
            AllocationPolicy::BestFit => self.best_fit(n_blocks),
        }
    }

    /// Best-fit : trouve le run libre le plus petit ≥ n_blocks.
    fn best_fit(&self, n_blocks: u64) -> Option<u64> {
        let runs = self.free_map.free_runs().ok()?;
        let mut best_start: Option<u64> = None;
        let mut best_len: u64 = u64::MAX;

        for run in &runs {
            if run.len >= n_blocks && run.len < best_len {
                best_len = run.len;
                best_start = Some(run.start);
            }
        }
        best_start
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HeapAllocator — allocateur principal
// ─────────────────────────────────────────────────────────────────────────────

/// Allocateur de blocs physiques dans le heap ExoFS.
///
/// Thread-safe via `SpinLock`.
pub struct HeapAllocator {
    state: SpinLock<HeapAllocatorState>,
    /// Offset de début du heap en octets (pour convertir bloc → DiskOffset).
    heap_start: u64,
    /// Taille d'un bloc en octets (toujours BLOCK_SIZE = 4096).
    block_size: u64,
    /// Politique d'allocation par défaut.
    policy: AllocationPolicy,
    /// Nombre total de blocs.
    total_blocks: u64,
    // ── Statistiques atomiques ──────────────────────────────────────────────
    n_allocs: AtomicU64,
    n_frees: AtomicU64,
    n_failures: AtomicU64,
    bytes_alloc: AtomicU64,
}

impl HeapAllocator {
    // ── Constructeur ─────────────────────────────────────────────────────────

    /// Crée un allocateur pour un heap de `total_blocks` blocs, commençant à `heap_start`.
    pub fn new(heap_start: u64, total_blocks: u64) -> Result<Self, ExofsError> {
        let state = HeapAllocatorState::new(total_blocks)?;
        Ok(Self {
            state: SpinLock::new(state),
            heap_start,
            block_size: BLOCK_SIZE,
            policy: AllocationPolicy::NextFit,
            total_blocks,
            n_allocs: AtomicU64::new(0),
            n_frees: AtomicU64::new(0),
            n_failures: AtomicU64::new(0),
            bytes_alloc: AtomicU64::new(0),
        })
    }

    /// Crée un allocateur avec une politique explicite.
    pub fn new_with_policy(
        heap_start: u64,
        total_blocks: u64,
        policy: AllocationPolicy,
    ) -> Result<Self, ExofsError> {
        let mut alloc = Self::new(heap_start, total_blocks)?;
        alloc.policy = policy;
        Ok(alloc)
    }

    // ── Allocation ────────────────────────────────────────────────────────────

    /// Alloue `size` octets — arrondit vers le haut au multiple de BLOCK_SIZE.
    ///
    /// # Règle ARITH-02 : checked_add/checked_mul pour tous les calculs.
    pub fn alloc(&self, size: u64) -> ExofsResult<Extent> {
        self.alloc_with_policy(size, self.policy)
    }

    /// Alloue avec une politique explicite.
    pub fn alloc_with_policy(&self, size: u64, policy: AllocationPolicy) -> ExofsResult<Extent> {
        if size == 0 {
            return Err(ExofsError::InvalidArgument);
        }

        // Arrondi vers le haut au multiple de block_size.
        let alloc_size = round_up_blocks(size, self.block_size)?;

        // Nombre de blocs nécessaires.
        let n_blocks = alloc_size / self.block_size;

        let mut state = self.state.lock();

        // Déclencher la coalescence si nécessaire.
        if HeapCoalescer::should_coalesce(&state.free_map, 15) {
            let opts = CoalesceOptions {
                apply: true,
                ..Default::default()
            };
            let _ = HeapCoalescer::run(&mut state.free_map, &opts);
        }

        // Chercher les blocs libres.
        let block_start = match state.find_blocks(n_blocks, policy) {
            Some(b) => b,
            None => {
                drop(state);
                self.n_failures.fetch_add(1, Ordering::Relaxed);
                STORAGE_STATS.inc_heap_alloc_failure();
                return Err(ExofsError::NoSpace);
            }
        };

        // Marquer comme occupé.
        state.free_map.mark_used(block_start, n_blocks);
        state.last_alloc = block_start.saturating_add(n_blocks);
        state.allocs_since_coalesce = state.allocs_since_coalesce.saturating_add(1);

        drop(state);

        // Calculer l'offset disque.
        // RÈGLE ARITH-02 : checked_mul pour bloc → octet.
        let block_offset = block_start
            .checked_mul(self.block_size)
            .ok_or(ExofsError::OffsetOverflow)?;
        let disk_offset = self
            .heap_start
            .checked_add(block_offset)
            .ok_or(ExofsError::OffsetOverflow)
            .map(DiskOffset)?;

        self.n_allocs.fetch_add(1, Ordering::Relaxed);
        self.bytes_alloc.fetch_add(alloc_size, Ordering::Relaxed);
        STORAGE_STATS.inc_heap_alloc(alloc_size);

        Ok(Extent {
            offset: disk_offset,
            size: alloc_size,
        })
    }

    // ── Libération ────────────────────────────────────────────────────────────

    /// Libère un extent précédemment alloué.
    ///
    /// # Règle ARITH-02 : checked_sub pour calculer le numéro de bloc.
    pub fn free_extent(&self, extent: Extent) -> ExofsResult<()> {
        if !extent.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }

        // L'offset doit être dans le heap.
        if extent.offset.0 < self.heap_start {
            return Err(ExofsError::InvalidArgument);
        }

        // Calculer le numéro de bloc relatif au heap.
        let rel_offset = extent
            .offset
            .0
            .checked_sub(self.heap_start)
            .ok_or(ExofsError::OffsetOverflow)?;

        if rel_offset % self.block_size != 0 {
            return Err(ExofsError::InvalidArgument);
        }

        let block_start = rel_offset / self.block_size;
        let n_blocks = extent.size / self.block_size;

        if block_start.saturating_add(n_blocks) > self.total_blocks {
            return Err(ExofsError::InvalidArgument);
        }

        {
            let mut state = self.state.lock();
            state.free_map.mark_free(block_start, n_blocks);

            // Tentative de fusion buddy.
            let _ = HeapCoalescer::try_merge_buddies(
                &mut state.free_map,
                block_start,
                14, // MAX_BUDDY_ORDER
            );
        }

        self.n_frees.fetch_add(1, Ordering::Relaxed);
        self.bytes_alloc.fetch_sub(
            self.bytes_alloc.load(Ordering::Relaxed).min(extent.size),
            Ordering::Relaxed,
        );
        STORAGE_STATS.inc_heap_free(extent.size);

        Ok(())
    }

    // ── Requêtes ─────────────────────────────────────────────────────────────

    /// Octets libres dans le heap.
    pub fn free_bytes(&self) -> u64 {
        let state = self.state.lock();
        state.free_map.free_blocks() * self.block_size
    }

    /// Octets alloués (approximatif — basé sur le compteur atomique).
    pub fn allocated_bytes(&self) -> u64 {
        self.bytes_alloc.load(Ordering::Relaxed)
    }

    /// Nombre total de blocs.
    #[inline]
    pub fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    /// Taille d'un bloc en octets.
    #[inline]
    pub fn block_size(&self) -> u64 {
        self.block_size
    }

    /// Nombre d'allocations réussies depuis le montage.
    pub fn n_allocs(&self) -> u64 {
        self.n_allocs.load(Ordering::Relaxed)
    }

    /// Nombre de libérations depuis le montage.
    pub fn n_frees(&self) -> u64 {
        self.n_frees.load(Ordering::Relaxed)
    }

    /// Nombre d'échecs d'allocation (espace insuffisant).
    pub fn n_failures(&self) -> u64 {
        self.n_failures.load(Ordering::Relaxed)
    }

    /// Taux d'utilisation en pourcentage (0..=100).
    pub fn usage_pct(&self) -> u64 {
        let state = self.state.lock();
        state.free_map.used_pct()
    }

    /// Taux de fragmentation en pourcentage (0..=100).
    pub fn fragmentation_pct(&self) -> u8 {
        let state = self.state.lock();
        state.free_map.fragmentation_pct()
    }

    /// Plus grand run libre disponible, en octets.
    pub fn largest_free_extent(&self) -> u64 {
        let state = self.state.lock();
        state.free_map.largest_free_run() * self.block_size
    }

    // ── Coalescence manuelle ─────────────────────────────────────────────────

    /// Force une passe de coalescence.
    pub fn coalesce(&self) -> ExofsResult<()> {
        let opts = CoalesceOptions {
            apply: true,
            ..Default::default()
        };
        let mut state = self.state.lock();
        HeapCoalescer::run(&mut state.free_map, &opts).map(|_| ())
    }

    // ── Réinitialisation ─────────────────────────────────────────────────────

    /// Libère tout l'espace (utilisé lors du formatage).
    pub fn reset_all_free(&self) -> ExofsResult<()> {
        let mut state = self.state.lock();
        *state = HeapAllocatorState::new(self.total_blocks)?;
        self.n_allocs.store(0, Ordering::Relaxed);
        self.n_frees.store(0, Ordering::Relaxed);
        self.n_failures.store(0, Ordering::Relaxed);
        self.bytes_alloc.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// Pré-réserve une plage de blocs (utilisé pour les métadonnées fixes).
    pub fn reserve_range(&self, start_block: u64, n_blocks: u64) {
        let mut state = self.state.lock();
        state.free_map.mark_used(start_block, n_blocks);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Arrondit `size` vers le haut au multiple de `block_size`.
///
/// # Règle ARITH-02 : checked_add.
#[inline]
fn round_up_blocks(size: u64, block_size: u64) -> ExofsResult<u64> {
    let mask = block_size
        .checked_sub(1)
        .ok_or(ExofsError::OffsetOverflow)?;
    size.checked_add(mask)
        .ok_or(ExofsError::OffsetOverflow)
        .map(|v| v & !mask)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const HEAP_START: u64 = 1024 * 1024; // 1 MB
    const N_BLOCKS: u64 = 1024; // 4 MB de heap

    fn make_allocator() -> HeapAllocator {
        HeapAllocator::new(HEAP_START, N_BLOCKS).unwrap()
    }

    #[test]
    fn test_alloc_basic() {
        let a = make_allocator();
        let ext = a.alloc(4096).unwrap();
        assert_eq!(ext.offset.0, HEAP_START);
        assert_eq!(ext.size, 4096);
        assert_eq!(a.n_allocs(), 1);
    }

    #[test]
    fn test_alloc_rounds_up() {
        let a = make_allocator();
        let ext = a.alloc(1).unwrap();
        assert_eq!(ext.size, 4096); // arrondi à BLOCK_SIZE
    }

    #[test]
    fn test_alloc_and_free() {
        let a = make_allocator();
        let ext = a.alloc(4096).unwrap();
        a.free_extent(ext).unwrap();
        assert_eq!(a.n_frees(), 1);
        // Réallouer la même zone.
        let ext2 = a.alloc(4096).unwrap();
        assert_eq!(ext2.offset.0, HEAP_START);
    }

    #[test]
    fn test_alloc_out_of_space() {
        let a = HeapAllocator::new(HEAP_START, 2).unwrap();
        let _e1 = a.alloc(4096).unwrap();
        let _e2 = a.alloc(4096).unwrap();
        assert!(a.alloc(4096).is_err());
        assert_eq!(a.n_failures(), 1);
    }

    #[test]
    fn test_free_bytes() {
        let a = make_allocator();
        let init_free = a.free_bytes();
        let _e = a.alloc(4096 * 10).unwrap();
        assert!(a.free_bytes() < init_free);
    }

    #[test]
    fn test_alloc_zero_fails() {
        let a = make_allocator();
        assert!(a.alloc(0).is_err());
    }
}
