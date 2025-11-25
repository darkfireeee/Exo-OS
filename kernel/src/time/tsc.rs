//! TSC (Time Stamp Counter) support
//! 
//! Provides high-resolution cycle counter for timing

use core::arch::x86_64::_rdtsc;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};

/// TSC frequency in Hz (calibrated at boot)
static TSC_FREQ_HZ: AtomicU64 = AtomicU64::new(0);

/// TSC start value (at boot)
static TSC_START: AtomicU64 = AtomicU64::new(0);

/// TSC calibration done
static TSC_CALIBRATED: AtomicBool = AtomicBool::new(false);

/// TSC structure for timing operations
pub struct Tsc;

impl Tsc {
    /// Read current TSC value
    #[inline]
    pub fn read() -> u64 {
        unsafe { _rdtsc() }
    }
    
    /// Get TSC frequency in Hz
    pub fn frequency() -> u64 {
        TSC_FREQ_HZ.load(Ordering::Relaxed)
    }
    
    /// Check if TSC is calibrated
    pub fn is_calibrated() -> bool {
        TSC_CALIBRATED.load(Ordering::Relaxed)
    }
    
    /// Convert cycles to nanoseconds
    pub fn cycles_to_ns(cycles: u64) -> u64 {
        let freq = Self::frequency();
        if freq == 0 {
            return 0;
        }
        // ns = cycles * 1_000_000_000 / freq
        cycles.saturating_mul(1_000_000_000) / freq
    }
    
    /// Convert nanoseconds to cycles
    pub fn ns_to_cycles(ns: u64) -> u64 {
        let freq = Self::frequency();
        if freq == 0 {
            return 0;
        }
        // cycles = ns * freq / 1_000_000_000
        ns.saturating_mul(freq) / 1_000_000_000
    }
    
    /// Get elapsed nanoseconds since boot
    pub fn elapsed_ns() -> u64 {
        let current = Self::read();
        let start = TSC_START.load(Ordering::Relaxed);
        let cycles = current.saturating_sub(start);
        Self::cycles_to_ns(cycles)
    }
    
    /// Benchmark a closure and return elapsed cycles
    pub fn benchmark<F, R>(f: F) -> (R, u64)
    where
        F: FnOnce() -> R,
    {
        let start = Self::read();
        let result = f();
        let end = Self::read();
        (result, end - start)
    }
}

/// Read TSC (shorthand)
#[inline]
pub fn read_tsc() -> u64 {
    Tsc::read()
}

/// Calibrate TSC using PIT or RTC
pub fn calibrate_tsc() -> Result<(), &'static str> {
    // Simple calibration using busy loop
    // In production, use PIT or HPET for accurate calibration
    
    // Assume 2.0 GHz for now (TODO: proper calibration)
    let estimated_freq = 2_000_000_000u64;
    
    TSC_FREQ_HZ.store(estimated_freq, Ordering::Relaxed);
    TSC_START.store(read_tsc(), Ordering::Relaxed);
    TSC_CALIBRATED.store(true, Ordering::Relaxed);
    
    Ok(())
}

/// Convert cycles to nanoseconds
pub fn cycles_to_ns(cycles: u64) -> u64 {
    Tsc::cycles_to_ns(cycles)
}

/// Convert nanoseconds to cycles
pub fn ns_to_cycles(ns: u64) -> u64 {
    Tsc::ns_to_cycles(ns)
}

/// Get elapsed nanoseconds since boot
pub fn elapsed_ns() -> u64 {
    Tsc::elapsed_ns()
}

/// Initialize TSC
pub fn init() {
    let _ = calibrate_tsc();
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_tsc_read() {
        let tsc1 = read_tsc();
        let tsc2 = read_tsc();
        assert!(tsc2 >= tsc1);
    }
}
