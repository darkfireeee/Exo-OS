// kernel/src/fs/exofs/storage/heap.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Heap ExoFS — façade unifiée pour l'allocation physique dans le heap disque
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le heap est la zone de données du volume : [HEAP_START, heap_end).
// Il contient les blobs et objets ExoFS sous forme d'extents contigus.
//
// Architecture :
//   ExofsHeap
//   ├── HeapAllocator      ← gestion des extents libres/alloués
//   └── HeapCoalescer      ← fusion des zones libres (appelé périodiquement)
//
// Règles respectées :
// - ARITH-02 : checked_add pour TOUS les calculs d'offset.
// - LOCK-04  : SpinLock uniquement quand nécessaire, pas d'I/O sous verrou.
// - ONDISK-03 : ce module est purement RAM.

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{ExofsError, ExofsResult, DiskOffset};
use crate::fs::exofs::storage::heap_allocator::{HeapAllocator, Extent, AllocationPolicy};
use crate::fs::exofs::storage::heap_coalesce::{HeapCoalescer, CoalesceOptions, CoalesceReport};
use crate::fs::exofs::storage::layout::{
    BLOCK_SIZE, HEAP_START_OFFSET, LayoutMap, heap_zone, blocks_for_bytes
};
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// HeapConfig — configuration du heap
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration initiale du heap ExoFS.
#[derive(Clone, Debug)]
pub struct HeapConfig {
    /// Offset de début du heap (octet).
    pub heap_start_offset: u64,
    /// Offset de fin du heap (octet, exclusif).
    pub heap_end_offset:   u64,
    /// Taille d'un bloc en octets (toujours BLOCK_SIZE = 4096).
    pub block_size:        u64,
    /// Politique d'allocation par défaut.
    pub policy:            AllocationPolicy,
    /// Seuil de fragmentation pour déclencher la coalescence automatique (%).
    pub auto_coalesce_threshold: u8,
}

impl HeapConfig {
    /// Construit la configuration depuis le LayoutMap.
    pub fn from_layout(lm: &LayoutMap) -> ExofsResult<Self> {
        let heap_end = lm.heap_start.0
            .checked_add(lm.heap_len)
            .ok_or(ExofsError::OffsetOverflow)?;
        Ok(Self {
            heap_start_offset:       lm.heap_start.0,
            heap_end_offset:         heap_end,
            block_size:              BLOCK_SIZE,
            policy:                  AllocationPolicy::NextFit,
            auto_coalesce_threshold: 15,
        })
    }

    /// Construit la configuration depuis la taille du disque.
    pub fn from_disk_size(disk_size_bytes: u64) -> ExofsResult<Self> {
        let lm = LayoutMap::new(disk_size_bytes)?;
        Self::from_layout(&lm)
    }

    /// Nombre de blocs dans le heap.
    pub fn total_blocks(&self) -> ExofsResult<u64> {
        let heap_len = self.heap_end_offset
            .checked_sub(self.heap_start_offset)
            .ok_or(ExofsError::OffsetOverflow)?;
        blocks_for_bytes(heap_len)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HeapStats — statistiques du heap
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot des statistiques du heap.
#[derive(Clone, Debug, Default)]
pub struct HeapStats {
    pub total_bytes:     u64,
    pub free_bytes:      u64,
    pub used_bytes:      u64,
    pub total_blocks:    u64,
    pub free_blocks:     u64,
    pub used_blocks:     u64,
    pub fragmentation:   u8,
    pub largest_free:    u64,
    pub n_allocs:        u64,
    pub n_frees:         u64,
    pub n_failures:      u64,
    pub usage_pct:       u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// ExofsHeap — façade principale
// ─────────────────────────────────────────────────────────────────────────────

/// Gestionnaire de heap disque ExoFS.
///
/// Fournit les opérations d'allocation/libération d'extents physiques.
pub struct ExofsHeap {
    allocator:          HeapAllocator,
    config:             HeapConfig,
    /// Coalescence automatique activée.
    auto_coalesce:      AtomicBool,
    /// Nombre de libérations depuis la dernière coalescence.
    frees_since_gc:     AtomicU64,
    /// Seuil pour déclencher la coalescence automatique.
    coalesce_threshold: u64,
}

impl ExofsHeap {
    // ── Constructeurs ─────────────────────────────────────────────────────────

    /// Crée un heap depuis la taille du disque.
    pub fn new(disk_size_bytes: u64) -> ExofsResult<Self> {
        let config = HeapConfig::from_disk_size(disk_size_bytes)?;
        Self::from_config(config)
    }

    /// Crée un heap depuis une configuration explicite.
    pub fn from_config(config: HeapConfig) -> ExofsResult<Self> {
        let total_blocks = config.total_blocks()?;
        let allocator = HeapAllocator::new_with_policy(
            config.heap_start_offset,
            total_blocks,
            config.policy,
        )?;
        Ok(Self {
            allocator,
            config,
            auto_coalesce:      AtomicBool::new(true),
            frees_since_gc:     AtomicU64::new(0),
            coalesce_threshold: 64,    // déclencher après 64 libérations
        })
    }

    // ── Allocation ────────────────────────────────────────────────────────────

    /// Alloue `size` octets dans le heap (arrondit à BLOCK_SIZE).
    ///
    /// # Règle ARITH-02 : délégué à HeapAllocator.
    #[inline]
    pub fn alloc(&self, size: u64) -> ExofsResult<Extent> {
        self.allocator.alloc(size)
    }

    /// Alloue avec une politique explicite.
    #[inline]
    pub fn alloc_with_policy(&self, size: u64, policy: AllocationPolicy) -> ExofsResult<Extent> {
        self.allocator.alloc_with_policy(size, policy)
    }

    /// Alloue un nombre de blocs fixes.
    ///
    /// # Règle ARITH-02 : checked_mul pour n_blocks × BLOCK_SIZE.
    pub fn alloc_blocks(&self, n_blocks: u64) -> ExofsResult<Extent> {
        let size = n_blocks
            .checked_mul(BLOCK_SIZE)
            .ok_or(ExofsError::OffsetOverflow)?;
        self.allocator.alloc(size)
    }

    // ── Libération ────────────────────────────────────────────────────────────

    /// Libère un extent.
    pub fn free(&self, extent: Extent) -> ExofsResult<()> {
        self.allocator.free_extent(extent)?;
        // Compteur pour coalescence automatique.
        let frees = self.frees_since_gc.fetch_add(1, Ordering::Relaxed) + 1;
        if self.auto_coalesce.load(Ordering::Relaxed) && frees >= self.coalesce_threshold {
            self.frees_since_gc.store(0, Ordering::Relaxed);
            let _ = self.allocator.coalesce();
        }
        Ok(())
    }

    // ── Coalescence ───────────────────────────────────────────────────────────

    /// Déclenche une passe de coalescence manuelle.
    pub fn coalesce(&self) -> ExofsResult<()> {
        self.allocator.coalesce()
    }

    /// Active ou désactive la coalescence automatique.
    #[inline]
    pub fn set_auto_coalesce(&self, enabled: bool) {
        self.auto_coalesce.store(enabled, Ordering::Relaxed);
    }

    // ── Requêtes ─────────────────────────────────────────────────────────────

    /// Octets libres dans le heap.
    #[inline]
    pub fn free_bytes(&self) -> u64 {
        self.allocator.free_bytes()
    }

    /// Octets alloués dans le heap.
    #[inline]
    pub fn allocated_bytes(&self) -> u64 {
        self.allocator.allocated_bytes()
    }

    /// Taille totale du heap en octets.
    pub fn total_bytes(&self) -> u64 {
        self.config.total_blocks()
            .map(|b| b * BLOCK_SIZE)
            .unwrap_or(0)
    }

    /// Nombre total de blocs.
    #[inline]
    pub fn total_blocks(&self) -> u64 {
        self.allocator.total_blocks()
    }

    /// Offset de début du heap.
    #[inline]
    pub fn heap_start(&self) -> DiskOffset {
        DiskOffset(self.config.heap_start_offset)
    }

    /// Taux d'utilisation en pourcentage (0..=100).
    pub fn usage_pct(&self) -> u64 {
        self.allocator.usage_pct()
    }

    /// Taux de fragmentation en pourcentage (0..=100).
    pub fn fragmentation_pct(&self) -> u8 {
        self.allocator.fragmentation_pct()
    }

    /// Plus grand extent libre disponible, en octets.
    pub fn largest_free_extent(&self) -> u64 {
        self.allocator.largest_free_extent()
    }

    /// Retourne les statistiques complètes du heap.
    pub fn stats(&self) -> HeapStats {
        let total_blocks  = self.total_blocks();
        let free_bytes    = self.free_bytes();
        let total_bytes   = self.total_bytes();
        let used_bytes    = total_bytes.saturating_sub(free_bytes);
        let free_blocks   = free_bytes / BLOCK_SIZE;
        let used_blocks   = total_blocks.saturating_sub(free_blocks);
        let usage_pct     = self.usage_pct();

        HeapStats {
            total_bytes,
            free_bytes,
            used_bytes,
            total_blocks,
            free_blocks,
            used_blocks,
            fragmentation: self.fragmentation_pct(),
            largest_free:  self.largest_free_extent(),
            n_allocs:      self.allocator.n_allocs(),
            n_frees:       self.allocator.n_frees(),
            n_failures:    self.allocator.n_failures(),
            usage_pct,
        }
    }

    // ── Validation d'un extent ────────────────────────────────────────────────

    /// Vérifie qu'un extent est dans les bornes du heap.
    pub fn extent_in_bounds(&self, extent: &Extent) -> bool {
        let start = extent.offset.0;
        let end   = match start.checked_add(extent.size) {
            Some(e) => e,
            None    => return false,
        };
        start >= self.config.heap_start_offset && end <= self.config.heap_end_offset
    }

    // ── Formatage ─────────────────────────────────────────────────────────────

    /// Remet tous les blocs comme libres (formatage).
    pub fn format(&self) -> ExofsResult<()> {
        self.allocator.reset_all_free()?;
        self.frees_since_gc.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// Réserve une plage de blocs (métadonnées fixes).
    ///
    /// # Règle ARITH-02 : checked_sub pour calculer le numéro de bloc.
    pub fn reserve_range(&self, start_offset: DiskOffset, size: u64) -> ExofsResult<()> {
        if start_offset.0 < self.config.heap_start_offset {
            return Err(ExofsError::InvalidArgument);
        }
        let rel = start_offset.0
            .checked_sub(self.config.heap_start_offset)
            .ok_or(ExofsError::OffsetOverflow)?;
        let block_start = rel / BLOCK_SIZE;
        let n_blocks    = size / BLOCK_SIZE;
        self.allocator.reserve_range(block_start, n_blocks);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions utilitaires de haut niveau
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule la taille minimale requise pour stocker `n_blobs` blobs de `blob_size` octets chacun.
///
/// # Règle ARITH-02 : checked_mul + checked_add.
pub fn required_heap_size(n_blobs: u64, blob_size: u64, overhead_pct: u64) -> ExofsResult<u64> {
    let data_size = n_blobs
        .checked_mul(blob_size)
        .ok_or(ExofsError::OffsetOverflow)?;

    // Overhead = data × overhead_pct / 100.
    let overhead = (data_size as u128 * overhead_pct as u128 / 100) as u64;

    data_size
        .checked_add(overhead)
        .ok_or(ExofsError::OffsetOverflow)
}

/// Vérifie que l'heap a assez d'espace pour `size` octets.
///
/// Retourne `Ok(())` si l'espace est suffisant, `Err(NoSpace)` sinon.
#[inline]
pub fn check_heap_space(heap: &ExofsHeap, size: u64) -> ExofsResult<()> {
    if heap.free_bytes() >= size {
        Ok(())
    } else {
        Err(ExofsError::NoSpace)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const DISK_SIZE: u64 = 64 * 1024 * 1024;   // 64 MB

    fn make_heap() -> ExofsHeap {
        ExofsHeap::new(DISK_SIZE).unwrap()
    }

    #[test]
    fn test_new_heap() {
        let h = make_heap();
        assert!(h.total_bytes() > 0);
        assert_eq!(h.free_bytes(), h.total_bytes());
    }

    #[test]
    fn test_alloc_and_free() {
        let h = make_heap();
        let ext = h.alloc(8192).unwrap();
        assert_eq!(ext.size, 8192);
        assert!(h.extent_in_bounds(&ext));
        h.free(ext).unwrap();
        assert_eq!(h.free_bytes(), h.total_bytes());
    }

    #[test]
    fn test_alloc_blocks() {
        let h = make_heap();
        let ext = h.alloc_blocks(3).unwrap();
        assert_eq!(ext.size, 3 * BLOCK_SIZE);
    }

    #[test]
    fn test_heap_start_offset() {
        let h = make_heap();
        assert_eq!(h.heap_start().0, HEAP_START_OFFSET);
    }

    #[test]
    fn test_stats() {
        let h   = make_heap();
        let s   = h.stats();
        assert_eq!(s.free_bytes, s.total_bytes);
        assert_eq!(s.used_bytes, 0);
    }

    #[test]
    fn test_check_heap_space() {
        let h = make_heap();
        assert!(check_heap_space(&h, 4096).is_ok());
        assert!(check_heap_space(&h, u64::MAX).is_err());
    }

    #[test]
    fn test_required_heap_size() {
        // 100 blobs × 4096 bytes + 10% overhead = 100 × 4096 × 1.1 = 450560.
        let s = required_heap_size(100, 4096, 10).unwrap();
        assert_eq!(s, 100 * 4096 + (100 * 4096 / 10));
    }

    #[test]
    fn test_extent_out_of_bounds() {
        let h = make_heap();
        let bad = Extent { offset: DiskOffset(0), size: 4096 };
        assert!(!h.extent_in_bounds(&bad));
    }
}
