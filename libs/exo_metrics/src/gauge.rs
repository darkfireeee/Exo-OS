//! Atomic gauge (arbitrary values)

use core::sync::atomic::{AtomicI64, Ordering};

/// Lock-free atomic gauge
pub struct Gauge {
    value: AtomicI64,
}

impl Gauge {
    pub const fn new() -> Self {
        Self {
            value: AtomicI64::new(0),
        }
    }
    
    /// Set value
    pub fn set(&self, val: i64) {
        self.value.store(val, Ordering::Relaxed);
    }
    
    /// Increment by 1
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Decrement by 1
    pub fn dec(&self) {
        self.value.fetch_sub(1, Ordering::Relaxed);
    }
    
    /// Add n
    pub fn add(&self, n: i64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }
    
    /// Subtract n
    pub fn sub(&self, n: i64) {
        self.value.fetch_sub(n, Ordering::Relaxed);
    }
    
    /// Get current value
    pub fn get(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }
}

impl Default for Gauge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_gauge() {
        let gauge = Gauge::new();
        assert_eq!(gauge.get(), 0);
        
        gauge.set(100);
        assert_eq!(gauge.get(), 100);
        
        gauge.inc();
        assert_eq!(gauge.get(), 101);
        
        gauge.dec();
        assert_eq!(gauge.get(), 100);
        
        gauge.add(50);
        assert_eq!(gauge.get(), 150);
        
        gauge.sub(30);
        assert_eq!(gauge.get(), 120);
    }
}
