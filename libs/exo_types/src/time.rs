// libs/exo_types/src/time.rs
//! Time types (monotonic and realtime timestamps)

use core::fmt;
use core::ops::{Add, Sub};

/// Nanoseconds since reference point
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Timestamp(u64);

impl Timestamp {
    /// Zero timestamp
    pub const ZERO: Self = Self(0);
    
    /// Nanoseconds per microsecond
    pub const NANOS_PER_MICRO: u64 = 1_000;
    
    /// Nanoseconds per millisecond
    pub const NANOS_PER_MILLI: u64 = 1_000_000;
    
    /// Nanoseconds per second
    pub const NANOS_PER_SEC: u64 = 1_000_000_000;
    
    /// Create timestamp from nanoseconds
    #[inline]
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }
    
    /// Create timestamp from microseconds
    #[inline]
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros * Self::NANOS_PER_MICRO)
    }
    
    /// Create timestamp from milliseconds
    #[inline]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis * Self::NANOS_PER_MILLI)
    }
    
    /// Create timestamp from seconds
    #[inline]
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs * Self::NANOS_PER_SEC)
    }
    
    /// Get nanoseconds
    #[inline]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    
    /// Get microseconds
    #[inline]
    pub const fn as_micros(self) -> u64 {
        self.0 / Self::NANOS_PER_MICRO
    }
    
    /// Get milliseconds
    #[inline]
    pub const fn as_millis(self) -> u64 {
        self.0 / Self::NANOS_PER_MILLI
    }
    
    /// Get seconds
    #[inline]
    pub const fn as_secs(self) -> u64 {
        self.0 / Self::NANOS_PER_SEC
    }
    
    /// Get current monotonic timestamp (syscall)
    pub fn now_monotonic() -> Self {
        // TODO: Real syscall to kernel
        Self::ZERO
    }
    
    /// Get current realtime timestamp (syscall)
    pub fn now_realtime() -> Self {
        // TODO: Real syscall to kernel
        Self::ZERO
    }
    
    /// Elapsed time since this timestamp
    #[inline]
    pub fn elapsed(self) -> Duration {
        Duration(Self::now_monotonic().0.saturating_sub(self.0))
    }
    
    /// Saturating add
    #[inline]
    pub const fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(duration.0))
    }
    
    /// Saturating sub
    #[inline]
    pub const fn saturating_sub(self, duration: Duration) -> Self {
        Self(self.0.saturating_sub(duration.0))
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;
    
    #[inline]
    fn add(self, rhs: Duration) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Self;
    
    #[inline]
    fn sub(self, rhs: Duration) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl Sub<Timestamp> for Timestamp {
    type Output = Duration;
    
    #[inline]
    fn sub(self, rhs: Timestamp) -> Duration {
        Duration(self.0 - rhs.0)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ns", self.0)
    }
}

/// Duration (time difference)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Duration(u64);

impl Duration {
    /// Zero duration
    pub const ZERO: Self = Self(0);
    
    /// Maximum duration
    pub const MAX: Self = Self(u64::MAX);
    
    /// Create duration from nanoseconds
    #[inline]
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }
    
    /// Create duration from microseconds
    #[inline]
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros * Timestamp::NANOS_PER_MICRO)
    }
    
    /// Create duration from milliseconds
    #[inline]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis * Timestamp::NANOS_PER_MILLI)
    }
    
    /// Create duration from seconds
    #[inline]
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs * Timestamp::NANOS_PER_SEC)
    }
    
    /// Get nanoseconds
    #[inline]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    
    /// Get microseconds
    #[inline]
    pub const fn as_micros(self) -> u64 {
        self.0 / Timestamp::NANOS_PER_MICRO
    }
    
    /// Get milliseconds
    #[inline]
    pub const fn as_millis(self) -> u64 {
        self.0 / Timestamp::NANOS_PER_MILLI
    }
    
    /// Get seconds
    #[inline]
    pub const fn as_secs(self) -> u64 {
        self.0 / Timestamp::NANOS_PER_SEC
    }
    
    /// Saturating add
    #[inline]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
    
    /// Saturating sub
    #[inline]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
    
    /// Sleep for this duration (syscall)
    pub fn sleep(self) {
        // TODO: Real syscall
        let _nanos = self.0;
    }
}

impl Add for Duration {
    type Output = Self;
    
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Sub for Duration {
    type Output = Self;
    
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 < Timestamp::NANOS_PER_MICRO {
            write!(f, "{}ns", self.0)
        } else if self.0 < Timestamp::NANOS_PER_MILLI {
            write!(f, "{}μs", self.as_micros())
        } else if self.0 < Timestamp::NANOS_PER_SEC {
            write!(f, "{}ms", self.as_millis())
        } else {
            write!(f, "{}s", self.as_secs())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_timestamp_creation() {
        assert_eq!(Timestamp::from_secs(1).as_nanos(), 1_000_000_000);
        assert_eq!(Timestamp::from_millis(500).as_micros(), 500_000);
    }
    
    #[test]
    fn test_duration_arithmetic() {
        let d1 = Duration::from_millis(100);
        let d2 = Duration::from_millis(50);
        assert_eq!((d1 + d2).as_millis(), 150);
        assert_eq!((d1 - d2).as_millis(), 50);
    }
    
    #[test]
    fn test_timestamp_arithmetic() {
        let t = Timestamp::from_secs(10);
        let d = Duration::from_secs(5);
        assert_eq!((t + d).as_secs(), 15);
        assert_eq!((t - d).as_secs(), 5);
    }
}
