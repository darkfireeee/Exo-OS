//! # High-Precision Time Management
//! 
//! System time sources with nanosecond precision:
//! - TSC (Time Stamp Counter) - CPU cycle counter
//! - HPET (High Precision Event Timer)
//! - APIC Timer
//! - Monotonic and real-time clocks

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use core::arch::x86_64::_rdtsc;

/// Global time manager
static TIME_MANAGER: TimeManager = TimeManager::new();

/// Time manager
pub struct TimeManager {
    /// TSC frequency in Hz
    tsc_freq: AtomicU64,
    
    /// TSC available
    tsc_available: AtomicBool,
    
    /// Boot time (Unix timestamp in microseconds)
    boot_time: AtomicU64,
    
    /// Monotonic time offset (nanoseconds since boot)
    monotonic_offset: AtomicU64,
}

impl TimeManager {
    /// Create new time manager
    const fn new() -> Self {
        Self {
            tsc_freq: AtomicU64::new(0),
            tsc_available: AtomicBool::new(false),
            boot_time: AtomicU64::new(0),
            monotonic_offset: AtomicU64::new(0),
        }
    }
    
    /// Initialize time sources
    pub fn init(&self) {
        // Detect TSC
        if Self::detect_tsc() {
            self.tsc_available.store(true, Ordering::Relaxed);
            
            // Calibrate TSC frequency
            let freq = Self::calibrate_tsc();
            self.tsc_freq.store(freq, Ordering::Relaxed);
        }
        
        // Set boot time (would read from RTC/CMOS)
        let boot_time = Self::read_rtc_time();
        self.boot_time.store(boot_time, Ordering::Relaxed);
    }
    
    /// Get monotonic time in nanoseconds (since boot)
    #[inline(always)]
    pub fn monotonic_ns(&self) -> u64 {
        if self.tsc_available.load(Ordering::Relaxed) {
            self.tsc_to_ns(self.read_tsc())
        } else {
            // Fallback to HPET or PIT
            self.monotonic_offset.load(Ordering::Relaxed)
        }
    }
    
    /// Get monotonic time in microseconds
    #[inline(always)]
    pub fn monotonic_us(&self) -> u64 {
        self.monotonic_ns() / 1000
    }
    
    /// Get monotonic time in milliseconds
    #[inline(always)]
    pub fn monotonic_ms(&self) -> u64 {
        self.monotonic_ns() / 1_000_000
    }
    
    /// Get monotonic time in seconds
    #[inline(always)]
    pub fn monotonic_sec(&self) -> u64 {
        self.monotonic_ns() / 1_000_000_000
    }
    
    /// Get real time (Unix timestamp in microseconds)
    #[inline(always)]
    pub fn realtime_us(&self) -> u64 {
        let boot = self.boot_time.load(Ordering::Relaxed);
        let uptime = self.monotonic_us();
        boot + uptime
    }
    
    /// Get real time in seconds
    #[inline(always)]
    pub fn realtime_sec(&self) -> u64 {
        self.realtime_us() / 1_000_000
    }
    
    /// Read TSC
    #[inline(always)]
    fn read_tsc(&self) -> u64 {
        unsafe { _rdtsc() }
    }
    
    /// Convert TSC ticks to nanoseconds
    #[inline(always)]
    fn tsc_to_ns(&self, ticks: u64) -> u64 {
        let freq = self.tsc_freq.load(Ordering::Relaxed);
        if freq == 0 {
            return 0;
        }
        
        // ns = ticks * 1_000_000_000 / freq
        // Avoid overflow with 128-bit arithmetic
        ((ticks as u128) * 1_000_000_000 / (freq as u128)) as u64
    }
    
    /// Detect TSC support
    fn detect_tsc() -> bool {
        // Check CPUID for TSC support
        #[cfg(target_arch = "x86_64")]
        {
            use core::arch::x86_64::__cpuid;
            unsafe {
                let cpuid = __cpuid(1);
                // TSC bit in EDX
                (cpuid.edx & (1 << 4)) != 0
            }
        }
        
        #[cfg(not(target_arch = "x86_64"))]
        false
    }
    
    /// Calibrate TSC frequency (Hz)
    fn calibrate_tsc() -> u64 {
        // Simplified calibration using PIT (Programmable Interval Timer)
        // Real implementation would use HPET or ACPI PM Timer
        
        // Assume 3 GHz for now (would measure actual frequency)
        3_000_000_000
    }
    
    /// Read RTC time (Unix timestamp in microseconds)
    fn read_rtc_time() -> u64 {
        // Would read from CMOS RTC
        // For now, return a fixed time (2024-01-01 00:00:00 UTC)
        1704067200 * 1_000_000
    }
}

/// Get global time manager
pub fn time_manager() -> &'static TimeManager {
    &TIME_MANAGER
}

/// Get current monotonic time in nanoseconds
#[inline(always)]
pub fn current_time_ns() -> u64 {
    TIME_MANAGER.monotonic_ns()
}

/// Get current monotonic time in microseconds
#[inline(always)]
pub fn current_time_us() -> u64 {
    TIME_MANAGER.monotonic_us()
}

/// Get current monotonic time in milliseconds
#[inline(always)]
pub fn current_time_ms() -> u64 {
    TIME_MANAGER.monotonic_ms()
}

/// Get current monotonic time in seconds
#[inline(always)]
pub fn current_time() -> u64 {
    TIME_MANAGER.monotonic_sec()
}

/// Get current real time (Unix timestamp in microseconds)
#[inline(always)]
pub fn realtime_us() -> u64 {
    TIME_MANAGER.realtime_us()
}

/// Get current real time (Unix timestamp in seconds)
#[inline(always)]
pub fn realtime() -> u64 {
    TIME_MANAGER.realtime_sec()
}

/// Duration measurement
pub struct Instant {
    ns: u64,
}

impl Instant {
    /// Create instant from current time
    pub fn now() -> Self {
        Self {
            ns: current_time_ns(),
        }
    }
    
    /// Elapsed time since instant
    pub fn elapsed(&self) -> Duration {
        let now = current_time_ns();
        Duration {
            ns: now.saturating_sub(self.ns),
        }
    }
    
    /// Elapsed time in nanoseconds
    #[inline(always)]
    pub fn elapsed_ns(&self) -> u64 {
        self.elapsed().as_nanos()
    }
    
    /// Elapsed time in microseconds
    #[inline(always)]
    pub fn elapsed_us(&self) -> u64 {
        self.elapsed().as_micros()
    }
    
    /// Elapsed time in milliseconds
    #[inline(always)]
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed().as_millis()
    }
}

/// Duration
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration {
    ns: u64,
}

impl Duration {
    /// Create duration from nanoseconds
    pub const fn from_nanos(ns: u64) -> Self {
        Self { ns }
    }
    
    /// Create duration from microseconds
    pub const fn from_micros(us: u64) -> Self {
        Self { ns: us * 1000 }
    }
    
    /// Create duration from milliseconds
    pub const fn from_millis(ms: u64) -> Self {
        Self { ns: ms * 1_000_000 }
    }
    
    /// Create duration from seconds
    pub const fn from_secs(secs: u64) -> Self {
        Self { ns: secs * 1_000_000_000 }
    }
    
    /// Get duration as nanoseconds
    pub const fn as_nanos(&self) -> u64 {
        self.ns
    }
    
    /// Get duration as microseconds
    pub const fn as_micros(&self) -> u64 {
        self.ns / 1000
    }
    
    /// Get duration as milliseconds
    pub const fn as_millis(&self) -> u64 {
        self.ns / 1_000_000
    }
    
    /// Get duration as seconds
    pub const fn as_secs(&self) -> u64 {
        self.ns / 1_000_000_000
    }
}

/// Initialize time subsystem
pub fn init() {
    TIME_MANAGER.init();
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_duration() {
        let d1 = Duration::from_secs(1);
        assert_eq!(d1.as_nanos(), 1_000_000_000);
        assert_eq!(d1.as_micros(), 1_000_000);
        assert_eq!(d1.as_millis(), 1_000);
        
        let d2 = Duration::from_millis(500);
        assert_eq!(d2.as_micros(), 500_000);
    }
    
    #[test]
    fn test_instant() {
        let start = Instant::now();
        
        // Simulate some work
        for _ in 0..1000 {
            core::hint::black_box(42);
        }
        
        let elapsed = start.elapsed();
        assert!(elapsed.as_nanos() > 0);
    }
}
