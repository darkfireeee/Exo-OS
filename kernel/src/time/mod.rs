//! Time management subsystem
//! 
//! Provides timekeeping, timers, and time-related utilities

pub mod tsc;
pub mod rtc;
pub mod hpet;
pub mod timer;
pub mod clock;

// Re-exports
pub use tsc::{Tsc, read_tsc, calibrate_tsc};
pub use rtc::{Rtc, read_rtc, DateTime};
pub use hpet::{Hpet, init_hpet};
pub use timer::{Timer, TimerId, set_timer, cancel_timer};
pub use clock::{SystemClock, Timestamp, Duration};

use core::sync::atomic::{AtomicU64, Ordering};

/// Global monotonic nanosecond counter
static MONOTONIC_NS: AtomicU64 = AtomicU64::new(0);

/// Global boot timestamp (UNIX time)
static BOOT_TIME: AtomicU64 = AtomicU64::new(0);

/// Initialize time subsystem
pub fn init() {
    // Initialize TSC
    tsc::init();
    
    // Initialize RTC
    rtc::init();
    
    // Try to initialize HPET (fallback to PIT if not available)
    if hpet::init().is_err() {
        // TODO: Initialize PIT as fallback
    }
    
    // Record boot time
    if let Some(dt) = rtc::read_rtc() {
        let unix_time = dt.to_unix_timestamp();
        BOOT_TIME.store(unix_time, Ordering::Relaxed);
    }
    
    // Initialize timers
    timer::init();
}

/// Get monotonic nanoseconds since boot
pub fn monotonic_ns() -> u64 {
    MONOTONIC_NS.load(Ordering::Relaxed)
}

/// Get system uptime in nanoseconds
pub fn uptime_ns() -> u64 {
    tsc::elapsed_ns()
}

/// Get current UNIX timestamp (seconds since epoch)
pub fn unix_timestamp() -> u64 {
    let boot = BOOT_TIME.load(Ordering::Relaxed);
    let uptime_secs = uptime_ns() / 1_000_000_000;
    boot + uptime_secs
}

/// Sleep for specified nanoseconds (busy wait)
pub fn busy_sleep_ns(ns: u64) {
    let start = read_tsc();
    let cycles = tsc::ns_to_cycles(ns);
    while read_tsc() - start < cycles {
        core::hint::spin_loop();
    }
}

/// Sleep for specified microseconds (busy wait)
pub fn busy_sleep_us(us: u64) {
    busy_sleep_ns(us * 1000)
}

/// Sleep for specified milliseconds (busy wait)
pub fn busy_sleep_ms(ms: u64) {
    busy_sleep_ns(ms * 1_000_000)
}
