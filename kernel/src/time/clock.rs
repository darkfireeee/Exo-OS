//! System clock and time utilities
//! 
//! Provides high-level time APIs

use super::{DateTime, uptime_ns, unix_timestamp};
use core::ops::{Add, Sub};

/// System timestamp (nanoseconds)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub u64);

impl Timestamp {
    /// Get current timestamp
    pub fn now() -> Self {
        Self(uptime_ns())
    }
    
    /// Get timestamp from nanoseconds
    pub fn from_ns(ns: u64) -> Self {
        Self(ns)
    }
    
    /// Get timestamp as nanoseconds
    pub fn as_ns(&self) -> u64 {
        self.0
    }
    
    /// Get timestamp as microseconds
    pub fn as_us(&self) -> u64 {
        self.0 / 1_000
    }
    
    /// Get timestamp as milliseconds
    pub fn as_ms(&self) -> u64 {
        self.0 / 1_000_000
    }
    
    /// Get timestamp as seconds
    pub fn as_secs(&self) -> u64 {
        self.0 / 1_000_000_000
    }
    
    /// Calculate duration since this timestamp
    pub fn elapsed(&self) -> Duration {
        let now = Self::now();
        Duration(now.0.saturating_sub(self.0))
    }
}

impl Add<Duration> for Timestamp {
    type Output = Timestamp;
    
    fn add(self, rhs: Duration) -> Self::Output {
        Timestamp(self.0.saturating_add(rhs.0))
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Timestamp;
    
    fn sub(self, rhs: Duration) -> Self::Output {
        Timestamp(self.0.saturating_sub(rhs.0))
    }
}

impl Sub<Timestamp> for Timestamp {
    type Output = Duration;
    
    fn sub(self, rhs: Timestamp) -> Self::Output {
        Duration(self.0.saturating_sub(rhs.0))
    }
}

/// Time duration (nanoseconds)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration(pub u64);

impl Duration {
    /// Zero duration
    pub const ZERO: Self = Self(0);
    
    /// Create duration from nanoseconds
    pub const fn from_ns(ns: u64) -> Self {
        Self(ns)
    }
    
    /// Create duration from microseconds
    pub const fn from_us(us: u64) -> Self {
        Self(us * 1_000)
    }
    
    /// Create duration from milliseconds
    pub const fn from_ms(ms: u64) -> Self {
        Self(ms * 1_000_000)
    }
    
    /// Create duration from seconds
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs * 1_000_000_000)
    }
    
    /// Get duration as nanoseconds
    pub const fn as_ns(&self) -> u64 {
        self.0
    }
    
    /// Get duration as microseconds
    pub const fn as_us(&self) -> u64 {
        self.0 / 1_000
    }
    
    /// Get duration as milliseconds
    pub const fn as_ms(&self) -> u64 {
        self.0 / 1_000_000
    }
    
    /// Get duration as seconds
    pub const fn as_secs(&self) -> u64 {
        self.0 / 1_000_000_000
    }
    
    /// Check if duration is zero
    pub const fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl Add for Duration {
    type Output = Duration;
    
    fn add(self, rhs: Self) -> Self::Output {
        Duration(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Duration {
    type Output = Duration;
    
    fn sub(self, rhs: Self) -> Self::Output {
        Duration(self.0.saturating_sub(rhs.0))
    }
}

/// System clock
pub struct SystemClock;

impl SystemClock {
    /// Get current monotonic timestamp
    pub fn now() -> Timestamp {
        Timestamp::now()
    }
    
    /// Get current UNIX time
    pub fn unix_time() -> u64 {
        unix_timestamp()
    }
    
    /// Get system uptime
    pub fn uptime() -> Duration {
        Duration::from_ns(uptime_ns())
    }
    
    /// Sleep for duration (busy wait)
    pub fn sleep(duration: Duration) {
        super::busy_sleep_ns(duration.as_ns());
    }
    
    /// Sleep for nanoseconds
    pub fn sleep_ns(ns: u64) {
        super::busy_sleep_ns(ns);
    }
    
    /// Sleep for microseconds
    pub fn sleep_us(us: u64) {
        super::busy_sleep_us(us);
    }
    
    /// Sleep for milliseconds
    pub fn sleep_ms(ms: u64) {
        super::busy_sleep_ms(ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_duration_conversions() {
        let dur = Duration::from_ms(1500);
        assert_eq!(dur.as_ms(), 1500);
        assert_eq!(dur.as_secs(), 1);
        assert_eq!(dur.as_us(), 1_500_000);
    }
    
    #[test]
    fn test_duration_arithmetic() {
        let d1 = Duration::from_ms(1000);
        let d2 = Duration::from_ms(500);
        
        assert_eq!((d1 + d2).as_ms(), 1500);
        assert_eq!((d1 - d2).as_ms(), 500);
    }
}
