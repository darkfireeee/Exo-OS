<<<<<<< Updated upstream
//! Time types for timestamps and durations
//!
//! Provides high-precision time types with nanosecond resolution.
//! All types are zero-cost abstractions with inline operations.

use core::fmt;
use core::ops::{Add, Sub, AddAssign, SubAssign};

/// Nanosecond timestamp since reference point
///
/// Represents an absolute point in time with nanosecond precision.
/// Can be monotonic (never goes backwards) or realtime (wall clock).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Timestamp(u64);

impl Timestamp {
    /// Zero timestamp (epoch)
    pub const ZERO: Self = Self(0);
    
    /// Maximum timestamp value
    pub const MAX: Self = Self(u64::MAX);
    
    /// Nanoseconds per microsecond
    pub const NANOS_PER_MICRO: u64 = 1_000;
    
    /// Nanoseconds per millisecond
    pub const NANOS_PER_MILLI: u64 = 1_000_000;
    
    /// Nanoseconds per second
    pub const NANOS_PER_SEC: u64 = 1_000_000_000;
    
    /// Nanoseconds per minute
    pub const NANOS_PER_MINUTE: u64 = 60 * Self::NANOS_PER_SEC;
    
    /// Nanoseconds per hour
    pub const NANOS_PER_HOUR: u64 = 60 * Self::NANOS_PER_MINUTE;
    
    /// Nanoseconds per day
    pub const NANOS_PER_DAY: u64 = 24 * Self::NANOS_PER_HOUR;
    
    /// Create timestamp from nanoseconds
    #[inline(always)]
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }
    
    /// Create timestamp from microseconds
    #[inline(always)]
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros.saturating_mul(Self::NANOS_PER_MICRO))
    }
    
    /// Create timestamp from milliseconds
    #[inline(always)]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis.saturating_mul(Self::NANOS_PER_MILLI))
    }
    
    /// Create timestamp from seconds
    #[inline(always)]
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs.saturating_mul(Self::NANOS_PER_SEC))
    }
    
    /// Create timestamp from seconds and nanoseconds
    #[inline(always)]
    pub const fn from_secs_nanos(secs: u64, nanos: u32) -> Self {
        Self(secs.saturating_mul(Self::NANOS_PER_SEC).saturating_add(nanos as u64))
    }
    
    /// Get nanoseconds component
    #[inline(always)]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    
    /// Get microseconds (rounded down)
    #[inline(always)]
    pub const fn as_micros(self) -> u64 {
        self.0 / Self::NANOS_PER_MICRO
    }
    
    /// Get milliseconds (rounded down)
    #[inline(always)]
    pub const fn as_millis(self) -> u64 {
        self.0 / Self::NANOS_PER_MILLI
    }
    
    /// Get seconds (rounded down)
    #[inline(always)]
    pub const fn as_secs(self) -> u64 {
        self.0 / Self::NANOS_PER_SEC
    }
    
    /// Get seconds and nanoseconds components
    #[inline(always)]
    pub const fn as_secs_nanos(self) -> (u64, u32) {
        let secs = self.0 / Self::NANOS_PER_SEC;
        let nanos = (self.0 % Self::NANOS_PER_SEC) as u32;
        (secs, nanos)
    }
    
    /// Get subsecond nanoseconds (0-999,999,999)
    #[inline(always)]
    pub const fn subsec_nanos(self) -> u32 {
        (self.0 % Self::NANOS_PER_SEC) as u32
    }
    
    /// Checked addition
    #[inline(always)]
    pub const fn checked_add(self, duration: Duration) -> Option<Self> {
        match self.0.checked_add(duration.0) {
            Some(nanos) => Some(Self(nanos)),
            None => None,
        }
    }
    
    /// Checked subtraction
    #[inline(always)]
    pub const fn checked_sub(self, duration: Duration) -> Option<Self> {
        match self.0.checked_sub(duration.0) {
            Some(nanos) => Some(Self(nanos)),
            None => None,
        }
    }
    
    /// Saturating addition
    #[inline(always)]
    pub const fn saturating_add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(duration.0))
    }
    
    /// Saturating subtraction
    #[inline(always)]
    pub const fn saturating_sub(self, duration: Duration) -> Self {
        Self(self.0.saturating_sub(duration.0))
    }
    
    /// Checked duration since another timestamp
    #[inline(always)]
    pub const fn checked_duration_since(self, earlier: Self) -> Option<Duration> {
        match self.0.checked_sub(earlier.0) {
            Some(nanos) => Some(Duration(nanos)),
            None => None,
        }
    }
    
    /// Saturating duration since another timestamp
    #[inline(always)]
    pub const fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration(self.0.saturating_sub(earlier.0))
    }
    
    /// Get current monotonic timestamp (syscall stub)
    ///
    /// NOTE: Returns ZERO until syscall layer is implemented.
    #[inline]
    pub fn now_monotonic() -> Self {
        // Real implementation: syscall::clock_gettime(CLOCK_MONOTONIC)
        Self::ZERO
    }
    
    /// Get current realtime timestamp (syscall stub)
    ///
    /// NOTE: Returns ZERO until syscall layer is implemented.
    #[inline]
    pub fn now_realtime() -> Self {
        // Real implementation: syscall::clock_gettime(CLOCK_REALTIME)
        Self::ZERO
    }
    
    /// Elapsed time since this timestamp (uses monotonic clock)
    #[inline]
    pub fn elapsed(self) -> Duration {
        Self::now_monotonic().saturating_duration_since(self)
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;
    
    #[inline(always)]
    fn add(self, rhs: Duration) -> Self {
        Self(self.0.wrapping_add(rhs.0))
    }
}

impl AddAssign<Duration> for Timestamp {
    #[inline(always)]
    fn add_assign(&mut self, rhs: Duration) {
        self.0 = self.0.wrapping_add(rhs.0);
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Self;
    
    #[inline(always)]
    fn sub(self, rhs: Duration) -> Self {
        Self(self.0.wrapping_sub(rhs.0))
    }
}

impl SubAssign<Duration> for Timestamp {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: Duration) {
        self.0 = self.0.wrapping_sub(rhs.0);
    }
}

impl Sub<Timestamp> for Timestamp {
    type Output = Duration;
    
    #[inline(always)]
    fn sub(self, rhs: Timestamp) -> Duration {
        Duration(self.0.wrapping_sub(rhs.0))
=======
//! Time-related types

use core::fmt;
use core::ops::{Add, Sub};

/// Timestamp in nanoseconds since epoch
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Timestamp(pub u64);

impl Timestamp {
    /// Create a new timestamp
    pub const fn new(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Get nanoseconds since epoch
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Get seconds since epoch
    pub const fn as_secs(self) -> u64 {
        self.0 / 1_000_000_000
    }

    /// Get milliseconds since epoch
    pub const fn as_millis(self) -> u64 {
        self.0 / 1_000_000
    }

    /// Get microseconds since epoch
    pub const fn as_micros(self) -> u64 {
        self.0 / 1_000
    }

    /// Create from seconds
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs * 1_000_000_000)
    }

    /// Create from milliseconds
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis * 1_000_000)
>>>>>>> Stashed changes
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
<<<<<<< Updated upstream
        let (secs, nanos) = self.as_secs_nanos();
        write!(f, "{}.{:09}s", secs, nanos)
    }
}

/// Time duration with nanosecond precision
///
/// Represents a time interval. Always positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Duration(u64);
=======
        write!(f, "{}s", self.as_secs())
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Self;

    fn sub(self, rhs: Duration) -> Self::Output {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl Sub for Timestamp {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Self::Output {
        Duration(self.0.saturating_sub(rhs.0))
    }
}

/// Duration in nanoseconds
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Duration(pub u64);
>>>>>>> Stashed changes

impl Duration {
    /// Zero duration
    pub const ZERO: Self = Self(0);
<<<<<<< Updated upstream
    
    /// Maximum duration
    pub const MAX: Self = Self(u64::MAX);
    
    /// One microsecond
    pub const MICROSECOND: Self = Self(Timestamp::NANOS_PER_MICRO);
    
    /// One millisecond
    pub const MILLISECOND: Self = Self(Timestamp::NANOS_PER_MILLI);
    
    /// One second
    pub const SECOND: Self = Self(Timestamp::NANOS_PER_SEC);
    
    /// One minute
    pub const MINUTE: Self = Self(Timestamp::NANOS_PER_MINUTE);
    
    /// One hour
    pub const HOUR: Self = Self(Timestamp::NANOS_PER_HOUR);
    
    /// One day
    pub const DAY: Self = Self(Timestamp::NANOS_PER_DAY);
    
    /// Create duration from nanoseconds
    #[inline(always)]
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }
    
    /// Create duration from microseconds
    #[inline(always)]
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros.saturating_mul(Timestamp::NANOS_PER_MICRO))
    }
    
    /// Create duration from milliseconds
    #[inline(always)]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis.saturating_mul(Timestamp::NANOS_PER_MILLI))
    }
    
    /// Create duration from seconds
    #[inline(always)]
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs.saturating_mul(Timestamp::NANOS_PER_SEC))
    }
    
    /// Create duration from seconds and nanoseconds
    #[inline(always)]
    pub const fn from_secs_nanos(secs: u64, nanos: u32) -> Self {
        Self(secs.saturating_mul(Timestamp::NANOS_PER_SEC).saturating_add(nanos as u64))
    }
    
    /// Get total nanoseconds
    #[inline(always)]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    
    /// Get total microseconds (rounded down)
    #[inline(always)]
    pub const fn as_micros(self) -> u64 {
        self.0 / Timestamp::NANOS_PER_MICRO
    }
    
    /// Get total milliseconds (rounded down)
    #[inline(always)]
    pub const fn as_millis(self) -> u64 {
        self.0 / Timestamp::NANOS_PER_MILLI
    }
    
    /// Get total seconds (rounded down)
    #[inline(always)]
    pub const fn as_secs(self) -> u64 {
        self.0 / Timestamp::NANOS_PER_SEC
    }
    
    /// Get seconds and nanoseconds components
    #[inline(always)]
    pub const fn as_secs_nanos(self) -> (u64, u32) {
        let secs = self.0 / Timestamp::NANOS_PER_SEC;
        let nanos = (self.0 % Timestamp::NANOS_PER_SEC) as u32;
        (secs, nanos)
    }
    
    /// Get subsecond nanoseconds (0-999,999,999)
    #[inline(always)]
    pub const fn subsec_nanos(self) -> u32 {
        (self.0 % Timestamp::NANOS_PER_SEC) as u32
    }
    
    /// Checked addition
    #[inline(always)]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(nanos) => Some(Self(nanos)),
            None => None,
        }
    }
    
    /// Checked subtraction
    #[inline(always)]
    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        match self.0.checked_sub(other.0) {
            Some(nanos) => Some(Self(nanos)),
            None => None,
        }
    }
    
    /// Checked multiplication
    #[inline(always)]
    pub const fn checked_mul(self, rhs: u32) -> Option<Self> {
        match self.0.checked_mul(rhs as u64) {
            Some(nanos) => Some(Self(nanos)),
            None => None,
        }
    }
    
    /// Checked division
    #[inline(always)]
    pub const fn checked_div(self, rhs: u32) -> Option<Self> {
        if rhs == 0 {
            None
        } else {
            Some(Self(self.0 / rhs as u64))
        }
    }
    
    /// Saturating addition
    #[inline(always)]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
    
    /// Saturating subtraction
    #[inline(always)]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
    
    /// Saturating multiplication
    #[inline(always)]
    pub const fn saturating_mul(self, rhs: u32) -> Self {
        Self(self.0.saturating_mul(rhs as u64))
    }
    
    /// Check if duration is zero
    #[inline(always)]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
    
    /// Sleep for this duration (syscall stub)
    ///
    /// NOTE: No-op until syscall layer is implemented.
    #[inline]
    pub fn sleep(self) {
        // Real implementation: syscall::nanosleep(self)
        let _ = self.0;
    }
}

impl Add for Duration {
    type Output = Self;
    
    #[inline(always)]
    fn add(self, rhs: Self) -> Self {
        Self(self.0.wrapping_add(rhs.0))
    }
}

impl AddAssign for Duration {
    #[inline(always)]
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.wrapping_add(rhs.0);
    }
}

impl Sub for Duration {
    type Output = Self;
    
    #[inline(always)]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.wrapping_sub(rhs.0))
    }
}

impl SubAssign for Duration {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: Self) {
        self.0 = self.0.wrapping_sub(rhs.0);
    }
=======

    /// Create a new duration
    pub const fn new(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Get nanoseconds
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Get seconds
    pub const fn as_secs(self) -> u64 {
        self.0 / 1_000_000_000
    }

    /// Get milliseconds
    pub const fn as_millis(self) -> u64 {
        self.0 / 1_000_000
    }

    /// Get microseconds
    pub const fn as_micros(self) -> u64 {
        self.0 / 1_000
    }

    /// Create from seconds
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs * 1_000_000_000)
    }

    /// Create from milliseconds
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis * 1_000_000)
    }

    /// Create from microseconds
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros * 1_000)
    }

    /// Create from nanoseconds
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Check if zero
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
>>>>>>> Stashed changes
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
<<<<<<< Updated upstream
        if self.0 < Timestamp::NANOS_PER_MICRO {
            write!(f, "{}ns", self.0)
        } else if self.0 < Timestamp::NANOS_PER_MILLI {
            write!(f, "{}.{:03}μs", self.as_micros(), (self.0 % Timestamp::NANOS_PER_MICRO))
        } else if self.0 < Timestamp::NANOS_PER_SEC {
            write!(f, "{}.{:03}ms", self.as_millis(), (self.0 % Timestamp::NANOS_PER_MILLI) / 1_000)
        } else {
            let (secs, nanos) = self.as_secs_nanos();
            write!(f, "{}.{:09}s", secs, nanos)
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;
    
    #[test]
    fn test_timestamp_creation() {
        assert_eq!(Timestamp::ZERO.as_nanos(), 0);
        assert_eq!(Timestamp::from_secs(1).as_nanos(), 1_000_000_000);
        assert_eq!(Timestamp::from_millis(500).as_micros(), 500_000);
        assert_eq!(Timestamp::from_micros(1000).as_nanos(), 1_000_000);
    }
    
    #[test]
    fn test_timestamp_from_secs_nanos() {
        let ts = Timestamp::from_secs_nanos(10, 500_000_000);
        assert_eq!(ts.as_secs(), 10);
        assert_eq!(ts.subsec_nanos(), 500_000_000);
    }
    
    #[test]
    fn test_timestamp_as_secs_nanos() {
        let ts = Timestamp::from_nanos(10_500_000_000);
        let (secs, nanos) = ts.as_secs_nanos();
        assert_eq!(secs, 10);
        assert_eq!(nanos, 500_000_000);
    }
    
    #[test]
    fn test_timestamp_checked_add() {
        let ts = Timestamp::from_secs(10);
        let dur = Duration::from_secs(5);
        assert_eq!(ts.checked_add(dur).unwrap().as_secs(), 15);
        
        let max_ts = Timestamp::MAX;
        assert!(max_ts.checked_add(Duration::SECOND).is_none());
    }
    
    #[test]
    fn test_timestamp_checked_sub() {
        let ts = Timestamp::from_secs(10);
        let dur = Duration::from_secs(5);
        assert_eq!(ts.checked_sub(dur).unwrap().as_secs(), 5);
        
        assert!(Timestamp::ZERO.checked_sub(Duration::SECOND).is_none());
    }
    
    #[test]
    fn test_timestamp_saturating_add() {
        let ts = Timestamp::from_secs(10);
        let dur = Duration::from_secs(5);
        assert_eq!(ts.saturating_add(dur).as_secs(), 15);
        
        let max_ts = Timestamp::MAX;
        assert_eq!(max_ts.saturating_add(Duration::SECOND), Timestamp::MAX);
    }
    
    #[test]
    fn test_timestamp_saturating_sub() {
        let ts = Timestamp::from_secs(10);
        let dur = Duration::from_secs(5);
        assert_eq!(ts.saturating_sub(dur).as_secs(), 5);
        
        assert_eq!(Timestamp::ZERO.saturating_sub(Duration::SECOND), Timestamp::ZERO);
    }
    
    #[test]
    fn test_timestamp_arithmetic() {
        let t = Timestamp::from_secs(10);
        let d = Duration::from_secs(5);
        assert_eq!((t + d).as_secs(), 15);
        assert_eq!((t - d).as_secs(), 5);
        
        let t1 = Timestamp::from_secs(20);
        let t2 = Timestamp::from_secs(10);
        let dur = t1 - t2;
        assert_eq!(dur.as_secs(), 10);
    }
    
    #[test]
    fn test_timestamp_assign_ops() {
        let mut t = Timestamp::from_secs(10);
        t += Duration::from_secs(5);
        assert_eq!(t.as_secs(), 15);
        
        t -= Duration::from_secs(3);
        assert_eq!(t.as_secs(), 12);
    }
    
    #[test]
    fn test_timestamp_display() {
        let ts = Timestamp::from_secs_nanos(42, 123_456_789);
        let s = std::format!("{}", ts);
        assert_eq!(s, "42.123456789s");
    }
    
    #[test]
    fn test_duration_creation() {
        assert_eq!(Duration::ZERO.as_nanos(), 0);
        assert_eq!(Duration::from_secs(1).as_nanos(), 1_000_000_000);
        assert_eq!(Duration::from_millis(500).as_micros(), 500_000);
    }
    
    #[test]
    fn test_duration_constants() {
        assert_eq!(Duration::MICROSECOND.as_nanos(), 1_000);
        assert_eq!(Duration::MILLISECOND.as_nanos(), 1_000_000);
        assert_eq!(Duration::SECOND.as_nanos(), 1_000_000_000);
        assert_eq!(Duration::MINUTE.as_secs(), 60);
        assert_eq!(Duration::HOUR.as_secs(), 3600);
        assert_eq!(Duration::DAY.as_secs(), 86400);
    }
    
    #[test]
    fn test_duration_from_secs_nanos() {
        let dur = Duration::from_secs_nanos(5, 500_000_000);
        assert_eq!(dur.as_secs(), 5);
        assert_eq!(dur.subsec_nanos(), 500_000_000);
    }
    
    #[test]
    fn test_duration_arithmetic() {
        let d1 = Duration::from_millis(100);
        let d2 = Duration::from_millis(50);
        assert_eq!((d1 + d2).as_millis(), 150);
        assert_eq!((d1 - d2).as_millis(), 50);
    }
    
    #[test]
    fn test_duration_checked_add() {
        let d1 = Duration::from_secs(10);
        let d2 = Duration::from_secs(5);
        assert_eq!(d1.checked_add(d2).unwrap().as_secs(), 15);
        
        assert!(Duration::MAX.checked_add(Duration::SECOND).is_none());
    }
    
    #[test]
    fn test_duration_checked_sub() {
        let d1 = Duration::from_secs(10);
        let d2 = Duration::from_secs(5);
        assert_eq!(d1.checked_sub(d2).unwrap().as_secs(), 5);
        
        assert!(Duration::ZERO.checked_sub(Duration::SECOND).is_none());
    }
    
    #[test]
    fn test_duration_checked_mul() {
        let dur = Duration::from_secs(10);
        assert_eq!(dur.checked_mul(5).unwrap().as_secs(), 50);
        
        assert!(Duration::MAX.checked_mul(2).is_none());
    }
    
    #[test]
    fn test_duration_checked_div() {
        let dur = Duration::from_secs(10);
        assert_eq!(dur.checked_div(5).unwrap().as_secs(), 2);
        assert!(dur.checked_div(0).is_none());
    }
    
    #[test]
    fn test_duration_saturating_add() {
        let d1 = Duration::from_secs(10);
        let d2 = Duration::from_secs(5);
        assert_eq!(d1.saturating_add(d2).as_secs(), 15);
        
        assert_eq!(Duration::MAX.saturating_add(Duration::SECOND), Duration::MAX);
    }
    
    #[test]
    fn test_duration_saturating_sub() {
        let d1 = Duration::from_secs(10);
        let d2 = Duration::from_secs(5);
        assert_eq!(d1.saturating_sub(d2).as_secs(), 5);
        
        assert_eq!(Duration::ZERO.saturating_sub(Duration::SECOND), Duration::ZERO);
    }
    
    #[test]
    fn test_duration_saturating_mul() {
        let dur = Duration::from_secs(10);
        assert_eq!(dur.saturating_mul(5).as_secs(), 50);
        
        assert_eq!(Duration::MAX.saturating_mul(2), Duration::MAX);
    }
    
    #[test]
    fn test_duration_is_zero() {
        assert!(Duration::ZERO.is_zero());
        assert!(!Duration::from_nanos(1).is_zero());
    }
    
    #[test]
    fn test_duration_assign_ops() {
        let mut d = Duration::from_secs(10);
        d += Duration::from_secs(5);
        assert_eq!(d.as_secs(), 15);
        
        d -= Duration::from_secs(3);
        assert_eq!(d.as_secs(), 12);
    }
    
    #[test]
    fn test_duration_display() {
        assert_eq!(std::format!("{}", Duration::from_nanos(500)), "500ns");
        assert_eq!(std::format!("{}", Duration::from_micros(500)), "500.000μs");
        assert_eq!(std::format!("{}", Duration::from_millis(500)), "500.000ms");
        assert_eq!(std::format!("{}", Duration::from_secs_nanos(5, 123_456_789)), "5.123456789s");
    }
    
    #[test]
    fn test_duration_ordering() {
        let d1 = Duration::from_secs(5);
        let d2 = Duration::from_secs(10);
        let d3 = Duration::from_secs(5);
        
        assert!(d1 < d2);
        assert!(d2 > d1);
        assert_eq!(d1, d3);
    }
    
    #[test]
    fn test_timestamp_ordering() {
        let t1 = Timestamp::from_secs(5);
        let t2 = Timestamp::from_secs(10);
        let t3 = Timestamp::from_secs(5);
        
        assert!(t1 < t2);
        assert!(t2 > t1);
        assert_eq!(t1, t3);
    }
    
    #[test]
    fn test_size() {
        assert_eq!(size_of::<Timestamp>(), size_of::<u64>());
        assert_eq!(size_of::<Duration>(), size_of::<u64>());
    }
    
    #[test]
    fn test_copy() {
        let ts1 = Timestamp::from_secs(42);
        let ts2 = ts1;
        assert_eq!(ts1.as_secs(), ts2.as_secs());
        
        let d1 = Duration::from_secs(42);
        let d2 = d1;
        assert_eq!(d1.as_secs(), d2.as_secs());
=======
        let secs = self.as_secs();
        let nanos = self.0 % 1_000_000_000;
        write!(f, "{}.{:09}s", secs, nanos)
    }
}

impl Add for Duration {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for Duration {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_sub(rhs.0))
    }
}

// Conversion from core::time::Duration
impl From<core::time::Duration> for Duration {
    fn from(d: core::time::Duration) -> Self {
        Self::from_nanos(d.as_nanos() as u64)
    }
}

impl From<Duration> for core::time::Duration {
    fn from(d: Duration) -> Self {
        core::time::Duration::from_nanos(d.0)
>>>>>>> Stashed changes
    }
}
