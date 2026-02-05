//! Atomic counter (monotonic increment)

use core::sync::atomic::{AtomicU64, Ordering};

/// Lock-free atomic counter
pub struct Counter {
    value: AtomicU64,
}

impl Counter {
    pub const fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
        }
    }
    
    /// Increment by 1
    pub fn inc(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Increment by n
    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }
    
    /// Get current value
    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
    
    /// Reset to zero
    pub fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_counter() {
        let counter = Counter::new();
        assert_eq!(counter.get(), 0);
        
        counter.inc();
        assert_eq!(counter.get(), 1);
        
        counter.add(10);
        assert_eq!(counter.get(), 11);
        
        counter.reset();
        assert_eq!(counter.get(), 0);
    }
}
