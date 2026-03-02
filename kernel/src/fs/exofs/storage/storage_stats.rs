// kernel/src/fs/exofs/storage/storage_stats.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Statistiques du sous-système storage
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

/// Compteurs du sous-système storage (séparés de ExofsStats pour granularité).
pub struct StorageStats {
    pub heap_allocs:          AtomicU64,
    pub heap_alloc_bytes:     AtomicU64,
    pub heap_free_calls:      AtomicU64,
    pub heap_free_bytes:      AtomicU64,
    pub object_writes:        AtomicU64,
    pub object_reads:         AtomicU64,
    pub blob_writes:          AtomicU64,
    pub blob_reads:           AtomicU64,
    pub partial_write_errors: AtomicU64,
    pub checksum_errors:      AtomicU64,
    pub alloc_failures:       AtomicU64,
    pub superblock_syncs:     AtomicU64,
}

impl StorageStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) } }
        Self {
            heap_allocs:          z!(),
            heap_alloc_bytes:     z!(),
            heap_free_calls:      z!(),
            heap_free_bytes:      z!(),
            object_writes:        z!(),
            object_reads:         z!(),
            blob_writes:          z!(),
            blob_reads:           z!(),
            partial_write_errors: z!(),
            checksum_errors:      z!(),
            alloc_failures:       z!(),
            superblock_syncs:     z!(),
        }
    }

    #[inline] pub fn inc_heap_allocs(&self)              { self.heap_allocs.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn add_heap_alloc_bytes(&self, n: u64) { self.heap_alloc_bytes.fetch_add(n, Ordering::Relaxed); }
    #[inline] pub fn inc_object_writes(&self)            { self.object_writes.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_object_reads(&self)             { self.object_reads.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_blob_writes(&self)              { self.blob_writes.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_blob_reads(&self)               { self.blob_reads.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_partial_write_errors(&self)     { self.partial_write_errors.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_checksum_errors(&self)          { self.checksum_errors.fetch_add(1, Ordering::Relaxed); }
    #[inline] pub fn inc_alloc_failures(&self)           { self.alloc_failures.fetch_add(1, Ordering::Relaxed); }

    /// Snapshot pour observabilité.
    pub fn snapshot(&self) -> StorageStatsSnapshot {
        StorageStatsSnapshot {
            heap_allocs:      self.heap_allocs.load(Ordering::Relaxed),
            heap_alloc_bytes: self.heap_alloc_bytes.load(Ordering::Relaxed),
            object_writes:    self.object_writes.load(Ordering::Relaxed),
            object_reads:     self.object_reads.load(Ordering::Relaxed),
            blob_writes:      self.blob_writes.load(Ordering::Relaxed),
            alloc_failures:   self.alloc_failures.load(Ordering::Relaxed),
            checksum_errors:  self.checksum_errors.load(Ordering::Relaxed),
        }
    }
}

/// Vue instantanée pour logging.
#[derive(Copy, Clone, Debug)]
pub struct StorageStatsSnapshot {
    pub heap_allocs:      u64,
    pub heap_alloc_bytes: u64,
    pub object_writes:    u64,
    pub object_reads:     u64,
    pub blob_writes:      u64,
    pub alloc_failures:   u64,
    pub checksum_errors:  u64,
}

/// Singleton global.
pub static STORAGE_STATS: StorageStats = StorageStats::new();
