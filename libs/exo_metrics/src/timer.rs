//! High-precision timer utilities

use crate::Histogram;

/// RDTSC-based timer (x86_64)
pub struct Timer {
    start: u64,
}

impl Timer {
    /// Start timer
    pub fn start() -> Self {
        Self {
            start: rdtsc(),
        }
    }
    
    /// Elapsed cycles
    pub fn elapsed(&self) -> u64 {
        rdtsc().saturating_sub(self.start)
    }
    
    /// Record to histogram
    pub fn record(self, histogram: &Histogram) {
        histogram.observe(self.elapsed());
    }
}

/// Read x86_64 timestamp counter
#[cfg(target_arch = "x86_64")]
fn rdtsc() -> u64 {
    unsafe {
        let low: u32;
        let high: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
        ((high as u64) << 32) | (low as u64)
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn rdtsc() -> u64 {
    0 // Fallback for non-x86_64
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_timer() {
        let timer = Timer::start();
        
        // Simulate work
        for _ in 0..1000 {
            core::hint::black_box(());
        }
        
        let elapsed = timer.elapsed();
        assert!(elapsed > 0);
    }
}
