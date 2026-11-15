//! cache_stats.rs — ExoFS cache atomic metrics (no_std).
//!
//! `CacheStats`: lock-free atomic counters for cache operations.
//! `CacheStatsSnapshot`: coherent copy for inspection.
//! Rules: ONDISK-03 (no AtomicU64 in repr(C)), ARITH-02.

use core::sync::atomic::{AtomicU64, Ordering};

/// Global instance of cache statistics.
pub static CACHE_STATS: CacheStats = CacheStats::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// CacheStats
// ─────────────────────────────────────────────────────────────────────────────

/// Atomic counters for cache operations.
///
/// All counters are monotonic (increasing) except `total_bytes` and
/// `dirty_bytes` which can decrease during evictions / flushes.
pub struct CacheStats {
    /// Total number of hits (blob found in cache).
    pub hits: AtomicU64,
    /// Total number of misses (blob absent from cache).
    pub misses: AtomicU64,
    /// Total number of evictions.
    pub evictions: AtomicU64,
    /// Total number of insertions.
    pub insertions: AtomicU64,
    /// Total number of manual invalidations.
    pub invalidations: AtomicU64,
    /// Number of warming (prefetch) operations.
    pub warmings: AtomicU64,
    /// Bytes currently in cache (approximate).
    pub total_bytes: AtomicU64,
    /// Dirty bytes awaiting writeback.
    pub dirty_bytes: AtomicU64,
    /// Number of triggered writeback flushes.
    pub flushes: AtomicU64,
    /// Number of shrink operations.
    pub shrinks: AtomicU64,
}

impl CacheStats {
    /// `const` constructor for `static`.
    pub const fn new_const() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            insertions: AtomicU64::new(0),
            invalidations: AtomicU64::new(0),
            warmings: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            dirty_bytes: AtomicU64::new(0),
            flushes: AtomicU64::new(0),
            shrinks: AtomicU64::new(0),
        }
    }

    // ── Recording ─────────────────────────────────────────────────────────────

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_eviction(&self, bytes: u64) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
        let cur = self.total_bytes.load(Ordering::Relaxed);
        self.total_bytes
            .store(cur.saturating_sub(bytes), Ordering::Relaxed);
    }

    pub fn record_insert(&self, bytes: u64) {
        self.insertions.fetch_add(1, Ordering::Relaxed);
        self.total_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_invalidation(&self, bytes: u64) {
        self.invalidations.fetch_add(1, Ordering::Relaxed);
        let cur = self.total_bytes.load(Ordering::Relaxed);
        self.total_bytes
            .store(cur.saturating_sub(bytes), Ordering::Relaxed);
    }

    pub fn record_warming(&self) {
        self.warmings.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_dirty_add(&self, bytes: u64) {
        self.dirty_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_dirty_flush(&self, bytes: u64) {
        self.flushes.fetch_add(1, Ordering::Relaxed);
        let cur = self.dirty_bytes.load(Ordering::Relaxed);
        self.dirty_bytes
            .store(cur.saturating_sub(bytes), Ordering::Relaxed);
    }

    pub fn record_shrink(&self) {
        self.shrinks.fetch_add(1, Ordering::Relaxed);
    }

    // ── Read ──────────────────────────────────────────────────────────────────

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }
    pub fn insertions(&self) -> u64 {
        self.insertions.load(Ordering::Relaxed)
    }
    pub fn invalidations(&self) -> u64 {
        self.invalidations.load(Ordering::Relaxed)
    }
    pub fn warmings(&self) -> u64 {
        self.warmings.load(Ordering::Relaxed)
    }
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }
    pub fn dirty_bytes(&self) -> u64 {
        self.dirty_bytes.load(Ordering::Relaxed)
    }
    pub fn flushes(&self) -> u64 {
        self.flushes.load(Ordering::Relaxed)
    }
    pub fn shrinks(&self) -> u64 {
        self.shrinks.load(Ordering::Relaxed)
    }

    /// Hit ratio percentage (0 if no accesses).
    pub fn hit_ratio_pct(&self) -> u64 {
        let h = self.hits.load(Ordering::Relaxed);
        let m = self.misses.load(Ordering::Relaxed);
        let total = h.wrapping_add(m);
        if total == 0 {
            return 0;
        }
        h * 100 / total
    }

    /// `true` if hit ratio is greater than or equal to `min_pct`.
    pub fn is_effective(&self, min_pct: u64) -> bool {
        self.hit_ratio_pct() >= min_pct
    }

    /// Resets all counters to zero.
    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
        self.insertions.store(0, Ordering::Relaxed);
        self.invalidations.store(0, Ordering::Relaxed);
        self.warmings.store(0, Ordering::Relaxed);
        self.total_bytes.store(0, Ordering::Relaxed);
        self.dirty_bytes.store(0, Ordering::Relaxed);
        self.flushes.store(0, Ordering::Relaxed);
        self.shrinks.store(0, Ordering::Relaxed);
    }

    /// Returns a coherent snapshot of the counters.
    pub fn snapshot(&self) -> CacheStatsSnapshot {
        CacheStatsSnapshot {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            insertions: self.insertions.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
            warmings: self.warmings.load(Ordering::Relaxed),
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            dirty_bytes: self.dirty_bytes.load(Ordering::Relaxed),
            flushes: self.flushes.load(Ordering::Relaxed),
            shrinks: self.shrinks.load(Ordering::Relaxed),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheStatsSnapshot
// ─────────────────────────────────────────────────────────────────────────────

/// Point-in-time copy of cache metrics.
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStatsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub insertions: u64,
    pub invalidations: u64,
    pub warmings: u64,
    pub total_bytes: u64,
    pub dirty_bytes: u64,
    pub flushes: u64,
    pub shrinks: u64,
}

impl CacheStatsSnapshot {
    pub fn total_accesses(&self) -> u64 {
        self.hits.wrapping_add(self.misses)
    }

    pub fn hit_ratio_pct(&self) -> u64 {
        let total = self.total_accesses();
        if total == 0 {
            return 0;
        }
        self.hits * 100 / total
    }

    /// `true` if no activity recorded.
    pub fn is_idle(&self) -> bool {
        self.total_accesses() == 0
    }

    /// Difference between two snapshots.
    pub fn delta(&self, prev: &CacheStatsSnapshot) -> CacheStatsDelta {
        CacheStatsDelta {
            d_hits: self.hits.wrapping_sub(prev.hits),
            d_misses: self.misses.wrapping_sub(prev.misses),
            d_evictions: self.evictions.wrapping_sub(prev.evictions),
            d_insertions: self.insertions.wrapping_sub(prev.insertions),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CacheStatsDelta
// ─────────────────────────────────────────────────────────────────────────────

/// Evolution of metrics between two snapshots.
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStatsDelta {
    pub d_hits: u64,
    pub d_misses: u64,
    pub d_evictions: u64,
    pub d_insertions: u64,
}

impl CacheStatsDelta {
    /// `true` if performance degraded (more misses than hits).
    pub fn is_degraded(&self) -> bool {
        self.d_misses > self.d_hits
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_zeros() {
        let s = CacheStats::new_const();
        assert_eq!(s.hits(), 0);
        assert_eq!(s.total_bytes(), 0);
    }

    #[test]
    fn test_record_hit_miss() {
        let s = CacheStats::new_const();
        s.record_hit();
        s.record_hit();
        s.record_miss();
        assert_eq!(s.hits(), 2);
        assert_eq!(s.misses(), 1);
    }

    #[test]
    fn test_hit_ratio_50pct() {
        let s = CacheStats::new_const();
        s.record_hit();
        s.record_miss();
        assert_eq!(s.hit_ratio_pct(), 50);
    }

    #[test]
    fn test_hit_ratio_zero_initially() {
        let s = CacheStats::new_const();
        assert_eq!(s.hit_ratio_pct(), 0);
    }

    #[test]
    fn test_record_insert_updates_bytes() {
        let s = CacheStats::new_const();
        s.record_insert(1024);
        assert_eq!(s.total_bytes(), 1024);
    }

    #[test]
    fn test_record_eviction_decreases_bytes() {
        let s = CacheStats::new_const();
        s.record_insert(2048);
        s.record_eviction(1024);
        assert_eq!(s.total_bytes(), 1024);
    }

    #[test]
    fn test_eviction_no_underflow() {
        let s = CacheStats::new_const();
        s.record_eviction(9999);
        assert_eq!(s.total_bytes(), 0);
    }

    #[test]
    fn test_reset_clears_all() {
        let s = CacheStats::new_const();
        s.record_hit();
        s.record_insert(512);
        s.record_eviction(256);
        s.reset();
        assert_eq!(s.hits(), 0);
        assert_eq!(s.total_bytes(), 0);
    }

    #[test]
    fn test_snapshot_coherent() {
        let s = CacheStats::new_const();
        s.record_hit();
        s.record_miss();
        s.record_insert(128);
        let snap = s.snapshot();
        assert_eq!(snap.hits, 1);
        assert_eq!(snap.misses, 1);
        assert_eq!(snap.total_bytes, 128);
    }

    #[test]
    fn test_snapshot_delta() {
        let s1 = CacheStatsSnapshot {
            hits: 10,
            misses: 5,
            ..Default::default()
        };
        let s2 = CacheStatsSnapshot {
            hits: 15,
            misses: 7,
            ..Default::default()
        };
        let d = s2.delta(&s1);
        assert_eq!(d.d_hits, 5);
        assert_eq!(d.d_misses, 2);
    }

    #[test]
    fn test_is_effective() {
        let s = CacheStats::new_const();
        for _ in 0..80 {
            s.record_hit();
        }
        for _ in 0..20 {
            s.record_miss();
        }
        assert!(s.is_effective(75));
        assert!(!s.is_effective(90));
    }

    #[test]
    fn test_dirty_tracking() {
        let s = CacheStats::new_const();
        s.record_dirty_add(512);
        assert_eq!(s.dirty_bytes(), 512);
        s.record_dirty_flush(512);
        assert_eq!(s.dirty_bytes(), 0);
    }
}

// ── CacheStats extensions ──────────────────────────────────────────────────

impl CacheStats {
    /// Copies the most important counters into an array [hits, misses, evictions, bytes].
    pub fn to_array(&self) -> [u64; 4] {
        [
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
            self.evictions.load(Ordering::Relaxed),
            self.total_bytes.load(Ordering::Relaxed),
        ]
    }

    /// `true` if the cache never served any request.
    pub fn is_cold(&self) -> bool {
        self.hits.load(Ordering::Relaxed) == 0 && self.misses.load(Ordering::Relaxed) == 0
    }

    /// Updates `total_bytes` directly (for external synchronization).
    pub fn set_total_bytes(&self, bytes: u64) {
        self.total_bytes.store(bytes, Ordering::Relaxed);
    }
}

impl CacheStatsSnapshot {
    /// `true` if hit ratio exceeds `min_pct`.
    pub fn is_effective(&self, min_pct: u64) -> bool {
        self.hit_ratio_pct() >= min_pct
    }

    /// Bytes per access (0 if no accesses).
    pub fn bytes_per_access(&self) -> u64 {
        let total = self.total_accesses();
        if total == 0 {
            return 0;
        }
        self.total_bytes / total
    }
}
