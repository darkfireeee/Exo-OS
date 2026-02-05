//! Histogram aggregation

use crate::Result;

/// Histogram aggregator
pub struct Histogram;

impl Histogram {
    /// Create histogram
    pub fn new() -> Self {
        Self
    }

    /// Record value
    pub fn record(&mut self, _value: u64) {
    }

    /// Get percentile
    pub fn percentile(&self, _p: f64) -> u64 {
        0
    }
}
