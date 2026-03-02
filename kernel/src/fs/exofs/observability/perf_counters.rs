//! perf_counters.rs — Compteurs de performance ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};

pub static PERF_COUNTERS: PerfCounters = PerfCounters::new_const();

/// Compteurs de performance granulaires.
pub struct PerfCounters {
    pub path_resolves:    AtomicU64,
    pub extent_tree_ops:  AtomicU64,
    pub blob_allocs:      AtomicU64,
    pub blob_frees:       AtomicU64,
    pub lock_contentions: AtomicU64,
    pub io_wait_ticks:    AtomicU64,
    pub epoch_barriers:   AtomicU64,
    pub checksum_ops:     AtomicU64,
    pub crypto_ops:       AtomicU64,
    pub dedup_checks:     AtomicU64,
}

impl PerfCounters {
    pub const fn new_const() -> Self {
        Self {
            path_resolves:    AtomicU64::new(0),
            extent_tree_ops:  AtomicU64::new(0),
            blob_allocs:      AtomicU64::new(0),
            blob_frees:       AtomicU64::new(0),
            lock_contentions: AtomicU64::new(0),
            io_wait_ticks:    AtomicU64::new(0),
            epoch_barriers:   AtomicU64::new(0),
            checksum_ops:     AtomicU64::new(0),
            crypto_ops:       AtomicU64::new(0),
            dedup_checks:     AtomicU64::new(0),
        }
    }

    pub fn inc_path_resolve(&self)           { self.path_resolves.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_extent_op(&self)              { self.extent_tree_ops.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_blob_alloc(&self)             { self.blob_allocs.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_blob_free(&self)              { self.blob_frees.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_lock_contention(&self)        { self.lock_contentions.fetch_add(1, Ordering::Relaxed); }
    pub fn add_io_wait(&self, ticks: u64)    { self.io_wait_ticks.fetch_add(ticks, Ordering::Relaxed); }
    pub fn inc_epoch_barrier(&self)          { self.epoch_barriers.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_checksum(&self)               { self.checksum_ops.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_crypto(&self)                 { self.crypto_ops.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_dedup_check(&self)            { self.dedup_checks.fetch_add(1, Ordering::Relaxed); }

    pub fn reset_all(&self) {
        self.path_resolves.store(0, Ordering::Relaxed);
        self.extent_tree_ops.store(0, Ordering::Relaxed);
        self.blob_allocs.store(0, Ordering::Relaxed);
        self.blob_frees.store(0, Ordering::Relaxed);
        self.lock_contentions.store(0, Ordering::Relaxed);
        self.io_wait_ticks.store(0, Ordering::Relaxed);
        self.epoch_barriers.store(0, Ordering::Relaxed);
        self.checksum_ops.store(0, Ordering::Relaxed);
        self.crypto_ops.store(0, Ordering::Relaxed);
        self.dedup_checks.store(0, Ordering::Relaxed);
    }
}
