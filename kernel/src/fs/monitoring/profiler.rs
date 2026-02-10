//! Profiler - Performance Profiling
//!
//! Profiles filesystem operations for performance analysis with histogram-based latency tracking.

use alloc::string::String;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use spin::RwLock;

/// Latency histogram bucket boundaries (microseconds)
const HISTOGRAM_BUCKETS: [u64; 10] = [
    1,      // < 1µs
    10,     // < 10µs
    50,     // < 50µs
    100,    // < 100µs
    500,    // < 500µs
    1000,   // < 1ms
    5000,   // < 5ms
    10000,  // < 10ms
    50000,  // < 50ms
    100000, // < 100ms
];

/// Operation profile with histogram
struct OperationProfile {
    /// Total invocations
    count: AtomicU64,
    /// Total time (nanoseconds)
    total_time_ns: AtomicU64,
    /// Minimum latency (nanoseconds)
    min_latency_ns: AtomicU64,
    /// Maximum latency (nanoseconds)
    max_latency_ns: AtomicU64,
    /// Histogram buckets
    histogram: [AtomicU64; 11], // 10 buckets + overflow
}

impl OperationProfile {
    const fn new() -> Self {
        const ATOMIC_ZERO: AtomicU64 = AtomicU64::new(0);
        Self {
            count: AtomicU64::new(0),
            total_time_ns: AtomicU64::new(0),
            min_latency_ns: AtomicU64::new(u64::MAX),
            max_latency_ns: AtomicU64::new(0),
            histogram: [ATOMIC_ZERO; 11],
        }
    }

    fn record(&self, latency_ns: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_time_ns.fetch_add(latency_ns, Ordering::Relaxed);

        // Update min
        let mut current_min = self.min_latency_ns.load(Ordering::Relaxed);
        while latency_ns < current_min {
            match self.min_latency_ns.compare_exchange_weak(
                current_min,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_min = x,
            }
        }

        // Update max
        let mut current_max = self.max_latency_ns.load(Ordering::Relaxed);
        while latency_ns > current_max {
            match self.max_latency_ns.compare_exchange_weak(
                current_max,
                latency_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => current_max = x,
            }
        }

        // Update histogram
        let latency_us = latency_ns / 1000; // Convert to microseconds
        let bucket = HISTOGRAM_BUCKETS.iter()
            .position(|&boundary| latency_us < boundary)
            .unwrap_or(HISTOGRAM_BUCKETS.len());

        self.histogram[bucket].fetch_add(1, Ordering::Relaxed);
    }

    fn avg_latency_us(&self) -> f64 {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        let total = self.total_time_ns.load(Ordering::Relaxed);
        (total as f64 / count as f64) / 1000.0
    }

    fn get_stats(&self) -> ProfileStats {
        let count = self.count.load(Ordering::Relaxed);

        ProfileStats {
            count,
            avg_latency_us: self.avg_latency_us(),
            min_latency_us: if count > 0 {
                self.min_latency_ns.load(Ordering::Relaxed) as f64 / 1000.0
            } else {
                0.0
            },
            max_latency_us: self.max_latency_ns.load(Ordering::Relaxed) as f64 / 1000.0,
            p50_latency_us: self.percentile(50),
            p95_latency_us: self.percentile(95),
            p99_latency_us: self.percentile(99),
        }
    }

    fn percentile(&self, p: u8) -> f64 {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }

        let target_count = (count as f64 * p as f64 / 100.0) as u64;
        let mut cumulative = 0;

        for (i, bucket_count_atomic) in self.histogram.iter().enumerate() {
            let bucket_count = bucket_count_atomic.load(Ordering::Relaxed);
            cumulative += bucket_count;

            if cumulative >= target_count {
                // Return middle of bucket
                if i == 0 {
                    return 0.5;
                } else if i < HISTOGRAM_BUCKETS.len() {
                    return HISTOGRAM_BUCKETS[i] as f64 / 2.0;
                } else {
                    return HISTOGRAM_BUCKETS[HISTOGRAM_BUCKETS.len() - 1] as f64;
                }
            }
        }

        0.0
    }
}

/// Profile statistics snapshot
#[derive(Debug, Clone, Copy)]
pub struct ProfileStats {
    pub count: u64,
    pub avg_latency_us: f64,
    pub min_latency_us: f64,
    pub max_latency_us: f64,
    pub p50_latency_us: f64,
    pub p95_latency_us: f64,
    pub p99_latency_us: f64,
}

/// Global profiler state
struct Profiler {
    profiles: RwLock<BTreeMap<String, OperationProfile>>,
    enabled: AtomicBool,
}

impl Profiler {
    const fn new() -> Self {
        Self {
            profiles: RwLock::new(BTreeMap::new()),
            enabled: AtomicBool::new(true),
        }
    }

    fn record(&self, name: &str, latency_ns: u64) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        // Fast path: existing profile
        {
            let profiles = self.profiles.read();
            if let Some(profile) = profiles.get(name) {
                profile.record(latency_ns);
                return;
            }
        }

        // Slow path: create new profile
        let mut profiles = self.profiles.write();
        let profile = profiles.entry(String::from(name))
            .or_insert_with(OperationProfile::new);
        profile.record(latency_ns);
    }

    fn get_stats(&self, name: &str) -> Option<ProfileStats> {
        let profiles = self.profiles.read();
        profiles.get(name).map(|p| p.get_stats())
    }

    fn get_all_stats(&self) -> BTreeMap<String, ProfileStats> {
        let profiles = self.profiles.read();
        profiles.iter()
            .map(|(name, profile)| (name.clone(), profile.get_stats()))
            .collect()
    }

    fn clear(&self) {
        let mut profiles = self.profiles.write();
        profiles.clear();
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}

/// Global profiler instance
static GLOBAL_PROFILER: Profiler = Profiler::new();

/// Profile a filesystem operation with automatic timing
pub fn profile_operation<F, R>(name: &str, f: F) -> R
where
    F: FnOnce() -> R,
{
    if !GLOBAL_PROFILER.enabled.load(Ordering::Relaxed) {
        return f();
    }

    let start = crate::time::uptime_ns();
    let result = f();
    let elapsed = crate::time::uptime_ns() - start;

    GLOBAL_PROFILER.record(name, elapsed);

    result
}

/// Manually record an operation latency
pub fn record_latency(name: &str, latency_ns: u64) {
    GLOBAL_PROFILER.record(name, latency_ns);
}

/// Get statistics for a specific operation
pub fn get_stats(name: &str) -> Option<ProfileStats> {
    GLOBAL_PROFILER.get_stats(name)
}

/// Get statistics for all profiled operations
pub fn get_all_stats() -> BTreeMap<String, ProfileStats> {
    GLOBAL_PROFILER.get_all_stats()
}

/// Clear all profiling data
pub fn clear() {
    GLOBAL_PROFILER.clear();
}

/// Enable/disable profiling
pub fn set_enabled(enabled: bool) {
    GLOBAL_PROFILER.set_enabled(enabled);
    log::debug!("Profiler {}", if enabled { "enabled" } else { "disabled" });
}

/// Check if profiling is enabled
pub fn is_enabled() -> bool {
    GLOBAL_PROFILER.enabled.load(Ordering::Relaxed)
}

/// Initialize profiler subsystem
pub fn init() {
    GLOBAL_PROFILER.set_enabled(true);
    log::debug!("Profiler subsystem initialized (histogram buckets: {})", HISTOGRAM_BUCKETS.len());
}
