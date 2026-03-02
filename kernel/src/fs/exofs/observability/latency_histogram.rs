//! latency_histogram.rs — Histogramme de latence I/O ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};

pub static LATENCY_HIST: LatencyHistogram = LatencyHistogram::new_const();

/// Buckets logarithmiques : [0-1µs), [1-10µs), [10-100µs), [100µs-1ms),
///   [1ms-10ms), [10ms-100ms), [100ms-1s), [>1s].
const N_BUCKETS: usize = 8;

pub struct LatencyHistogram {
    buckets: [AtomicU64; N_BUCKETS],
    total_samples: AtomicU64,
    sum_ticks:     AtomicU64,
}

impl LatencyHistogram {
    pub const fn new_const() -> Self {
        Self {
            buckets: [
                AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0),
            ],
            total_samples: AtomicU64::new(0),
            sum_ticks:     AtomicU64::new(0),
        }
    }

    /// Enregistre une latence en nanosecondes.
    pub fn record_ns(&self, ns: u64) {
        let bucket = if      ns <       1_000 { 0 }
                     else if ns <      10_000 { 1 }
                     else if ns <     100_000 { 2 }
                     else if ns <   1_000_000 { 3 }
                     else if ns <  10_000_000 { 4 }
                     else if ns < 100_000_000 { 5 }
                     else if ns < 1_000_000_000 { 6 }
                     else                     { 7 };
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.total_samples.fetch_add(1, Ordering::Relaxed);
        self.sum_ticks.fetch_add(ns, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> [u64; N_BUCKETS] {
        core::array::from_fn(|i| self.buckets[i].load(Ordering::Relaxed))
    }

    pub fn avg_ns(&self) -> u64 {
        let n = self.total_samples.load(Ordering::Relaxed);
        if n == 0 { 0 } else { self.sum_ticks.load(Ordering::Relaxed) / n }
    }

    pub fn total_samples(&self) -> u64 { self.total_samples.load(Ordering::Relaxed) }
}
