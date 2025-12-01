// Capability Cache
//
// LRU cache for FD → Capability lookups.
// Target: 90%+ hit rate, ~10 cycles on hit vs 50 cycles on miss.

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, Ordering};

/// Cache entry
#[derive(Debug, Clone, Copy)]
struct CacheEntry {
    /// File descriptor
    fd: i32,
    /// Capability ID
    capability_id: u64,
    /// Timestamp for LRU
    timestamp: u64,
}

/// LRU Capability Cache
pub struct CapabilityCache {
    /// Cache entries (fixed size array for performance)
    entries: [Option<CacheEntry>; Self::CACHE_SIZE],
    /// Next timestamp for LRU
    next_timestamp: AtomicU64,

    // Statistics
    hits: AtomicU64,
    misses: AtomicU64,
}

impl CapabilityCache {
    /// Cache size (must be power of 2 for fast modulo)
    pub const CACHE_SIZE: usize = 64;

    /// Create a new empty cache
    pub const fn new() -> Self {
        Self {
            entries: [None; Self::CACHE_SIZE],
            next_timestamp: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Lookup capability ID for FD (fast path)
    #[inline]
    pub fn get(&self, fd: i32) -> Option<u64> {
        // Hash FD to cache index
        let index = (fd as usize) & (Self::CACHE_SIZE - 1);

        if let Some(entry) = &self.entries[index] {
            if entry.fd == fd {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(entry.capability_id);
            }
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert FD → Capability mapping
    #[inline]
    pub fn insert(&mut self, fd: i32, capability_id: u64) {
        let index = (fd as usize) & (Self::CACHE_SIZE - 1);
        let timestamp = self.next_timestamp.fetch_add(1, Ordering::Relaxed);

        self.entries[index] = Some(CacheEntry {
            fd,
            capability_id,
            timestamp,
        });
    }

    /// Invalidate cache entry for FD
    pub fn invalidate(&mut self, fd: i32) {
        let index = (fd as usize) & (Self::CACHE_SIZE - 1);

        if let Some(entry) = &self.entries[index] {
            if entry.fd == fd {
                self.entries[index] = None;
            }
        }
    }

    /// Clear entire cache
    pub fn clear(&mut self) {
        for entry in &mut self.entries {
            *entry = None;
        }
    }

    /// Get cache hit rate (0.0 to 1.0)
    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;

        if total == 0 {
            return 0.0;
        }

        hits as f64 / total as f64
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);

        CacheStats {
            hits,
            misses,
            hit_rate: self.hit_rate(),
            size: Self::CACHE_SIZE,
        }
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
    }
}

/// Cache statistics
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub size: usize,
}

impl Default for CapabilityCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-CPU capability caches
use spin::RwLock;

static CPU_CACHES: [RwLock<CapabilityCache>; 8] = [
    RwLock::new(CapabilityCache::new()),
    RwLock::new(CapabilityCache::new()),
    RwLock::new(CapabilityCache::new()),
    RwLock::new(CapabilityCache::new()),
    RwLock::new(CapabilityCache::new()),
    RwLock::new(CapabilityCache::new()),
    RwLock::new(CapabilityCache::new()),
    RwLock::new(CapabilityCache::new()),
];

/// Get cache for current CPU
pub fn current_cache() -> &'static RwLock<CapabilityCache> {
    // TODO: Get actual CPU ID
    let cpu_id = 0;
    &CPU_CACHES[cpu_id % 8]
}

/// Get aggregate cache statistics
pub fn aggregate_stats() -> CacheStats {
    let mut total_hits = 0;
    let mut total_misses = 0;

    for cache in &CPU_CACHES {
        let stats = cache.read().stats();
        total_hits += stats.hits;
        total_misses += stats.misses;
    }

    let total = total_hits + total_misses;
    let hit_rate = if total > 0 {
        total_hits as f64 / total as f64
    } else {
        0.0
    };

    CacheStats {
        hits: total_hits,
        misses: total_misses,
        hit_rate,
        size: CapabilityCache::CACHE_SIZE * 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_basic() {
        let mut cache = CapabilityCache::new();

        cache.insert(5, 0x1000);
        assert_eq!(cache.get(5), Some(0x1000));
        assert_eq!(cache.get(6), None);
    }

    #[test]
    fn test_cache_hit_rate() {
        let mut cache = CapabilityCache::new();

        cache.insert(5, 0x1000);

        let _ = cache.get(5); // Hit
        let _ = cache.get(5); // Hit
        let _ = cache.get(6); // Miss

        assert_eq!(cache.stats().hits, 2);
        assert_eq!(cache.stats().misses, 1);
        assert!((cache.hit_rate() - 0.666).abs() < 0.01);
    }
}
