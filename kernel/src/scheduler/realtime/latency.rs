//! Latency tracking for real-time tasks

use core::sync::atomic::{AtomicU64, Ordering};

/// Latency tracker
pub struct LatencyTracker {
    /// Maximum observed latency (ns)
    max_latency_ns: AtomicU64,
    /// Total latency (ns)
    total_latency_ns: AtomicU64,
    /// Number of samples
    samples: AtomicU64,
}

impl LatencyTracker {
    pub const fn new() -> Self {
        Self {
            max_latency_ns: AtomicU64::new(0),
            total_latency_ns: AtomicU64::new(0),
            samples: AtomicU64::new(0),
        }
    }
    
    /// Record latency sample
    pub fn record(&self, latency_ns: u64) {
        self.total_latency_ns.fetch_add(latency_ns, Ordering::Relaxed);
        self.samples.fetch_add(1, Ordering::Relaxed);
        
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
    }
    
    /// Get maximum latency
    pub fn max_latency(&self) -> u64 {
        self.max_latency_ns.load(Ordering::Relaxed)
    }
    
    /// Get average latency
    pub fn average_latency(&self) -> u64 {
        let total = self.total_latency_ns.load(Ordering::Relaxed);
        let count = self.samples.load(Ordering::Relaxed);
        if count == 0 {
            0
        } else {
            total / count
        }
    }
    
    /// Reset statistics
    pub fn reset(&self) {
        self.max_latency_ns.store(0, Ordering::Relaxed);
        self.total_latency_ns.store(0, Ordering::Relaxed);
        self.samples.store(0, Ordering::Relaxed);
    }
}

impl Default for LatencyTracker {
    fn default() -> Self {
        Self::new()
    }
}
