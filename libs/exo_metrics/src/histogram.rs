//! Histogram for percentile distribution

use core::sync::atomic::{AtomicU64, Ordering};

/// Fixed buckets histogram
pub struct Histogram {
    buckets: [AtomicU64; 16],
    boundaries: [u64; 16],
    count: AtomicU64,
    sum: AtomicU64,
}

impl Histogram {
    /// Create histogram with logarithmic buckets
    pub fn new() -> Self {
        Self {
            buckets: [
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
            ],
            boundaries: [
                1, 2, 4, 8, 16, 32, 64, 128,
                256, 512, 1024, 2048, 4096, 8192, 16384, u64::MAX,
            ],
            count: AtomicU64::new(0),
            sum: AtomicU64::new(0),
        }
    }
    
    /// Observe a value
    pub fn observe(&self, value: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum.fetch_add(value, Ordering::Relaxed);
        
        for (i, &boundary) in self.boundaries.iter().enumerate() {
            if value <= boundary {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
                break;
            }
        }
    }
    
    /// Get total count
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
    
    /// Get sum of all observations
    pub fn sum(&self) -> u64 {
        self.sum.load(Ordering::Relaxed)
    }
    
    /// Get bucket counts
    pub fn buckets(&self) -> [(u64, u64); 16] {
        let mut result = [(0, 0); 16];
        for (i, bucket) in self.buckets.iter().enumerate() {
            result[i] = (self.boundaries[i], bucket.load(Ordering::Relaxed));
        }
        result
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_histogram() {
        let hist = Histogram::new();
        
        hist.observe(5);
        hist.observe(10);
        hist.observe(100);
        
        assert_eq!(hist.count(), 3);
        assert_eq!(hist.sum(), 115);
        
        let buckets = hist.buckets();
        assert!(buckets[0].1 > 0); // Some values in first buckets
    }
}
