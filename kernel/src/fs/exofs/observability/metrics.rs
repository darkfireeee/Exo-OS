//! metrics.rs — Métriques globales ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};

pub static EXOFS_METRICS: ExofsMetrics = ExofsMetrics::new_const();

/// Snapshot de toutes les métriques ExoFS.
#[derive(Clone, Debug, Default)]
pub struct MetricsSnapshot {
    pub reads:         u64,
    pub writes:        u64,
    pub read_bytes:    u64,
    pub write_bytes:   u64,
    pub cache_hits:    u64,
    pub cache_misses:  u64,
    pub errors:        u64,
    pub gc_runs:       u64,
    pub dedup_saves:   u64,
    pub epoch_commits: u64,
}

/// Compteurs atomiques globaux ExoFS.
pub struct ExofsMetrics {
    pub reads:         AtomicU64,
    pub writes:        AtomicU64,
    pub read_bytes:    AtomicU64,
    pub write_bytes:   AtomicU64,
    pub cache_hits:    AtomicU64,
    pub cache_misses:  AtomicU64,
    pub errors:        AtomicU64,
    pub gc_runs:       AtomicU64,
    pub dedup_saves:   AtomicU64,
    pub epoch_commits: AtomicU64,
}

impl ExofsMetrics {
    pub const fn new_const() -> Self {
        Self {
            reads:         AtomicU64::new(0),
            writes:        AtomicU64::new(0),
            read_bytes:    AtomicU64::new(0),
            write_bytes:   AtomicU64::new(0),
            cache_hits:    AtomicU64::new(0),
            cache_misses:  AtomicU64::new(0),
            errors:        AtomicU64::new(0),
            gc_runs:       AtomicU64::new(0),
            dedup_saves:   AtomicU64::new(0),
            epoch_commits: AtomicU64::new(0),
        }
    }

    pub fn inc_read(&self, bytes: u64) {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.read_bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    pub fn inc_write(&self, bytes: u64) {
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.write_bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    pub fn inc_cache_hit(&self)    { self.cache_hits.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_cache_miss(&self)   { self.cache_misses.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_error(&self)        { self.errors.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_gc(&self)           { self.gc_runs.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_dedup_save(&self, bytes: u64) { self.dedup_saves.fetch_add(bytes, Ordering::Relaxed); }
    pub fn inc_epoch_commit(&self) { self.epoch_commits.fetch_add(1, Ordering::Relaxed); }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            reads:         self.reads.load(Ordering::Relaxed),
            writes:        self.writes.load(Ordering::Relaxed),
            read_bytes:    self.read_bytes.load(Ordering::Relaxed),
            write_bytes:   self.write_bytes.load(Ordering::Relaxed),
            cache_hits:    self.cache_hits.load(Ordering::Relaxed),
            cache_misses:  self.cache_misses.load(Ordering::Relaxed),
            errors:        self.errors.load(Ordering::Relaxed),
            gc_runs:       self.gc_runs.load(Ordering::Relaxed),
            dedup_saves:   self.dedup_saves.load(Ordering::Relaxed),
            epoch_commits: self.epoch_commits.load(Ordering::Relaxed),
        }
    }
}
