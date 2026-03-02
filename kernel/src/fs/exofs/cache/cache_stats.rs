//! CacheStats — métriques du cache ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};

pub static CACHE_STATS: CacheStats = CacheStats::new_const();

pub struct CacheStats {
    pub hits:        AtomicU64,
    pub misses:      AtomicU64,
    pub evictions:   AtomicU64,
    pub warmings:    AtomicU64,
    pub dirty_pages: AtomicU64,
    pub total_bytes: AtomicU64,
}

impl CacheStats {
    pub const fn new_const() -> Self {
        Self {
            hits:        AtomicU64::new(0),
            misses:      AtomicU64::new(0),
            evictions:   AtomicU64::new(0),
            warmings:    AtomicU64::new(0),
            dirty_pages: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
        }
    }

    pub fn record_hit(&self) { self.hits.fetch_add(1, Ordering::Relaxed); }
    pub fn record_miss(&self) { self.misses.fetch_add(1, Ordering::Relaxed); }
    pub fn record_eviction(&self, bytes: u64) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
        self.total_bytes.fetch_sub(bytes.min(self.total_bytes.load(Ordering::Relaxed)), Ordering::Relaxed);
    }
    pub fn record_insert(&self, bytes: u64) {
        self.total_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn hit_ratio_percent(&self) -> u64 {
        let total = self.hits.load(Ordering::Relaxed) + self.misses.load(Ordering::Relaxed);
        if total == 0 { return 0; }
        self.hits.load(Ordering::Relaxed) * 100 / total
    }

    #[derive(Clone, Copy, Debug)]
    pub struct Snapshot {
        pub hits: u64, pub misses: u64, pub evictions: u64, pub total_bytes: u64,
    }

    pub fn snapshot(&self) -> CacheStatsSnapshot {
        CacheStatsSnapshot {
            hits:        self.hits.load(Ordering::Relaxed),
            misses:      self.misses.load(Ordering::Relaxed),
            evictions:   self.evictions.load(Ordering::Relaxed),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CacheStatsSnapshot {
    pub hits:        u64,
    pub misses:      u64,
    pub evictions:   u64,
    pub total_bytes: u64,
}
