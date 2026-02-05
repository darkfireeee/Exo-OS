//! Production-grade metrics system for Exo-OS
//!
//! Lock-free atomics-based telemetry:
//! - Counters (monotonic increment)
//! - Gauges (arbitrary values)
//! - Histograms (percentile distribution)
//! - Prometheus exporter

#![no_std]

extern crate alloc;

use core::sync::atomic::{AtomicU64, Ordering};

pub mod counter;
pub mod gauge;
pub mod histogram;
pub mod timer;
pub mod registry;
pub mod exporter;

pub use counter::Counter;
pub use gauge::Gauge;
pub use histogram::Histogram;
pub use timer::Timer;
pub use registry::{MetricsRegistry, Metric, MetricEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsError {
    NotFound,
    InvalidType,
    ExportFailed,
    BufferFull,
}

pub type Result<T> = core::result::Result<T, MetricsError>;

/// Global timestamp (nanoseconds since boot)
static BOOT_TIME_NS: AtomicU64 = AtomicU64::new(0);

pub fn set_boot_time(ns: u64) {
    BOOT_TIME_NS.store(ns, Ordering::Relaxed);
}

pub fn current_time_ns() -> u64 {
    BOOT_TIME_NS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_types() {
        let _ = MetricType::Counter;
        let _ = MetricType::Gauge;
        let _ = MetricType::Histogram;
    }
}
