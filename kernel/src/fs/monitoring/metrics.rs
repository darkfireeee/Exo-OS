//! Performance Metrics - Filesystem Performance Tracking
//!
//! Collects and aggregates filesystem performance metrics.

use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

/// Filesystem performance metrics
pub struct FsMetrics {
    // File operations
    pub open_count: AtomicU64,
    pub close_count: AtomicU64,
    pub read_count: AtomicU64,
    pub write_count: AtomicU64,
    pub read_bytes: AtomicU64,
    pub write_bytes: AtomicU64,

    // Directory operations
    pub readdir_count: AtomicU64,
    pub mkdir_count: AtomicU64,
    pub rmdir_count: AtomicU64,

    // Cache metrics
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,

    // Error metrics
    pub errors: AtomicU32,

    // Latency tracking (nanoseconds)
    pub read_latency_sum: AtomicU64,
    pub write_latency_sum: AtomicU64,
}

impl FsMetrics {
    pub const fn new() -> Self {
        Self {
            open_count: AtomicU64::new(0),
            close_count: AtomicU64::new(0),
            read_count: AtomicU64::new(0),
            write_count: AtomicU64::new(0),
            read_bytes: AtomicU64::new(0),
            write_bytes: AtomicU64::new(0),
            readdir_count: AtomicU64::new(0),
            mkdir_count: AtomicU64::new(0),
            rmdir_count: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            errors: AtomicU32::new(0),
            read_latency_sum: AtomicU64::new(0),
            write_latency_sum: AtomicU64::new(0),
        }
    }

    /// Record a read operation
    pub fn record_read(&self, bytes: usize, latency_ns: u64) {
        self.read_count.fetch_add(1, Ordering::Relaxed);
        self.read_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.read_latency_sum.fetch_add(latency_ns, Ordering::Relaxed);
    }

    /// Record a write operation
    pub fn record_write(&self, bytes: usize, latency_ns: u64) {
        self.write_count.fetch_add(1, Ordering::Relaxed);
        self.write_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.write_latency_sum.fetch_add(latency_ns, Ordering::Relaxed);
    }

    /// Record cache hit
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record cache miss
    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Get average read latency (microseconds)
    pub fn avg_read_latency_us(&self) -> f64 {
        let count = self.read_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        let sum = self.read_latency_sum.load(Ordering::Relaxed);
        (sum as f64 / count as f64) / 1000.0
    }

    /// Get average write latency (microseconds)
    pub fn avg_write_latency_us(&self) -> f64 {
        let count = self.write_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        let sum = self.write_latency_sum.load(Ordering::Relaxed);
        (sum as f64 / count as f64) / 1000.0
    }

    /// Get cache hit rate
    pub fn cache_hit_rate(&self) -> f64 {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            return 0.0;
        }
        hits as f64 / total as f64
    }
}

/// Global metrics instance
static GLOBAL_METRICS: FsMetrics = FsMetrics::new();

/// Get global metrics
pub fn global_metrics() -> &'static FsMetrics {
    &GLOBAL_METRICS
}

/// Initialize metrics subsystem
pub fn init() {
    log::debug!("Metrics subsystem initialized");
}
