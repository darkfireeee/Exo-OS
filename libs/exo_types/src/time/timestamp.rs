//! Timestamp type for monotonic and real-time clocks
//!
//! High-precision timestamps with nanosecond resolution.

use core::fmt;
use core::ops::{Add, Sub};
use super::duration::Duration;

/// Timestamp kind (monotonic vs real-time)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum TimestampKind {
    /// Monotonic clock (never goes backwards, not affected by wall-clock changes)
    Monotonic = 0,
    /// Real-time clock (wall-clock time, can jump backwards/forwards)
    Realtime = 1,
}

/// High-precision timestamp
///
/// Represents a point in time with nanosecond precision.
/// Stores seconds + nanoseconds for maximum range and precision.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    /// Seconds since epoch
    seconds: i64,
    /// Nanoseconds (0-999,999,999)
    nanos: u32,
    /// Clock type
    kind: TimestampKind,
}

impl Timestamp {
    /// Maximum nanoseconds value (1 billion - 1)
    pub const MAX_NANOS: u32 = 999_999_999;

    /// Nanoseconds per second
    pub const NANOS_PER_SEC: u32 = 1_000_000_000;

    /// Zero timestamp (epoch)
    pub const ZERO_MONOTONIC: Self = Self {
        seconds: 0,
        nanos: 0,
        kind: TimestampKind::Monotonic,
    };

    /// Zero timestamp (epoch, realtime)
    pub const ZERO_REALTIME: Self = Self {
        seconds: 0,
        nanos: 0,
        kind: TimestampKind::Realtime,
    };

    /// Create new timestamp with validation
    ///
    /// # Panics
    /// Panics in debug mode if nanos >= 1_000_000_000
    #[inline(always)]
    pub const fn new(seconds: i64, nanos: u32, kind: TimestampKind) -> Self {
        debug_assert!(nanos < Self::NANOS_PER_SEC, "Nanoseconds must be < 1_000_000_000");
        Self { seconds, nanos, kind }
    }

    /// Create monotonic timestamp
    #[inline(always)]
    pub const fn monotonic(seconds: i64, nanos: u32) -> Self {
        Self::new(seconds, nanos, TimestampKind::Monotonic)
    }

    /// Create realtime timestamp
    #[inline(always)]
    pub const fn realtime(seconds: i64, nanos: u32) -> Self {
        Self::new(seconds, nanos, TimestampKind::Realtime)
    }

    /// Try to create timestamp, returning None if invalid
    #[inline]
    pub const fn try_new(seconds: i64, nanos: u32, kind: TimestampKind) -> Option<Self> {
        if nanos < Self::NANOS_PER_SEC {
            Some(Self { seconds, nanos, kind })
        } else {
            None
        }
    }

    /// Create from total nanoseconds
    #[inline]
    pub const fn from_nanos(total_nanos: i64, kind: TimestampKind) -> Self {
        let seconds = total_nanos / (Self::NANOS_PER_SEC as i64);
        let nanos = (total_nanos % (Self::NANOS_PER_SEC as i64)).abs() as u32;
        Self { seconds, nanos, kind }
    }

    /// Create from milliseconds
    #[inline]
    pub const fn from_millis(millis: i64, kind: TimestampKind) -> Self {
        let seconds = millis / 1000;
        let nanos = ((millis % 1000).abs() as u32) * 1_000_000;
        Self { seconds, nanos, kind }
    }

    /// Create from microseconds
    #[inline]
    pub const fn from_micros(micros: i64, kind: TimestampKind) -> Self {
        let seconds = micros / 1_000_000;
        let nanos = ((micros % 1_000_000).abs() as u32) * 1_000;
        Self { seconds, nanos, kind }
    }

    /// Get seconds component
    #[inline(always)]
    pub const fn seconds(self) -> i64 {
        self.seconds
    }

    /// Get nanoseconds component (0-999,999,999)
    #[inline(always)]
    pub const fn nanos(self) -> u32 {
        self.nanos
    }

    /// Get timestamp kind
    #[inline(always)]
    pub const fn kind(self) -> TimestampKind {
        self.kind
    }

    /// Convert to total nanoseconds
    #[inline]
    pub const fn as_nanos(self) -> i64 {
        self.seconds
            .saturating_mul(Self::NANOS_PER_SEC as i64)
            .saturating_add(self.nanos as i64)
    }

    /// Convert to milliseconds
    #[inline]
    pub const fn as_millis(self) -> i64 {
        self.seconds
            .saturating_mul(1000)
            .saturating_add((self.nanos / 1_000_000) as i64)
    }

    /// Convert to microseconds
    #[inline]
    pub const fn as_micros(self) -> i64 {
        self.seconds
            .saturating_mul(1_000_000)
            .saturating_add((self.nanos / 1_000) as i64)
    }

    /// Check if timestamp is zero
    #[inline(always)]
    pub const fn is_zero(self) -> bool {
        self.seconds == 0 && self.nanos == 0
    }

    /// Checked addition with duration
    #[inline]
    pub const fn checked_add(self, duration: Duration) -> Option<Self> {
        let total_nanos = (self.nanos as i64).wrapping_add(duration.nanos() as i64);
        let extra_secs = total_nanos / (Self::NANOS_PER_SEC as i64);
        let new_nanos = (total_nanos % (Self::NANOS_PER_SEC as i64)) as u32;

        match self.seconds.checked_add(duration.seconds()) {
            Some(s) => match s.checked_add(extra_secs) {
                Some(secs) => Some(Self {
                    seconds: secs,
                    nanos: new_nanos,
                    kind: self.kind,
                }),
                None => None,
            },
            None => None,
        }
    }

    /// Checked subtraction with duration
    #[inline]
    pub const fn checked_sub(self, duration: Duration) -> Option<Self> {
        let total_nanos = (self.nanos as i64).wrapping_sub(duration.nanos() as i64);
        let (extra_secs, new_nanos) = if total_nanos < 0 {
            (-1, (total_nanos + (Self::NANOS_PER_SEC as i64)) as u32)
        } else {
            (0, total_nanos as u32)
        };

        match self.seconds.checked_sub(duration.seconds()) {
            Some(s) => match s.checked_add(extra_secs) {
                Some(secs) => Some(Self {
                    seconds: secs,
                    nanos: new_nanos,
                    kind: self.kind,
                }),
                None => None,
            },
            None => None,
        }
    }

    /// Duration since another timestamp
    ///
    /// Returns None if timestamps are of different kinds or if result would overflow
    #[inline]
    pub const fn duration_since(self, earlier: Timestamp) -> Option<Duration> {
        if self.kind as u8 != earlier.kind as u8 {
            return None;
        }

        let sec_diff = match self.seconds.checked_sub(earlier.seconds) {
            Some(diff) => diff,
            None => return None,
        };

        let (secs, nanos) = if self.nanos >= earlier.nanos {
            (sec_diff, self.nanos - earlier.nanos)
        } else {
            match sec_diff.checked_sub(1) {
                Some(s) => (s, Self::NANOS_PER_SEC + self.nanos - earlier.nanos),
                None => return None,
            }
        };

        Duration::try_new(secs, nanos)
    }

    /// Saturating addition with duration
    #[inline]
    pub fn saturating_add(self, duration: Duration) -> Self {
        self.checked_add(duration).unwrap_or_else(|| {
            if duration.is_positive() {
                Self {
                    seconds: i64::MAX,
                    nanos: Self::MAX_NANOS,
                    kind: self.kind,
                }
            } else {
                self
            }
        })
    }

    /// Saturating subtraction with duration
    #[inline]
    pub fn saturating_sub(self, duration: Duration) -> Self {
        self.checked_sub(duration).unwrap_or_else(|| {
            if duration.is_positive() {
                Self {
                    seconds: i64::MIN,
                    nanos: 0,
                    kind: self.kind,
                }
            } else {
                self
            }
        })
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timestamp")
            .field("kind", &self.kind)
            .field("seconds", &self.seconds)
            .field("nanos", &self.nanos)
            .finish()
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:09}s ({:?})", self.seconds, self.nanos, self.kind)
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    #[inline]
    fn add(self, duration: Duration) -> Self {
        self.saturating_add(duration)
    }
}

impl Sub<Duration> for Timestamp {
    type Output = Self;

    #[inline]
    fn sub(self, duration: Duration) -> Self {
        self.saturating_sub(duration)
    }
}

impl Sub<Timestamp> for Timestamp {
    type Output = Option<Duration>;

    #[inline]
    fn sub(self, other: Timestamp) -> Option<Duration> {
        self.duration_since(other)
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn test_timestamp_creation() {
        let ts = Timestamp::monotonic(100, 500_000_000);
        assert_eq!(ts.seconds(), 100);
        assert_eq!(ts.nanos(), 500_000_000);
        assert_eq!(ts.kind(), TimestampKind::Monotonic);

        let rt = Timestamp::realtime(200, 123_456_789);
        assert_eq!(rt.kind(), TimestampKind::Realtime);
    }

    #[test]
    fn test_timestamp_try_new() {
        assert!(Timestamp::try_new(10, 500_000_000, TimestampKind::Monotonic).is_some());
        assert!(Timestamp::try_new(10, 1_000_000_000, TimestampKind::Monotonic).is_none());
        assert!(Timestamp::try_new(10, 1_500_000_000, TimestampKind::Monotonic).is_none());
    }

    #[test]
    fn test_timestamp_from_nanos() {
        let ts = Timestamp::from_nanos(1_500_000_000, TimestampKind::Monotonic);
        assert_eq!(ts.seconds(), 1);
        assert_eq!(ts.nanos(), 500_000_000);

        let ts2 = Timestamp::from_nanos(999, TimestampKind::Monotonic);
        assert_eq!(ts2.seconds(), 0);
        assert_eq!(ts2.nanos(), 999);
    }

    #[test]
    fn test_timestamp_from_millis() {
        let ts = Timestamp::from_millis(1500, TimestampKind::Monotonic);
        assert_eq!(ts.seconds(), 1);
        assert_eq!(ts.nanos(), 500_000_000);
    }

    #[test]
    fn test_timestamp_from_micros() {
        let ts = Timestamp::from_micros(1_500_000, TimestampKind::Monotonic);
        assert_eq!(ts.seconds(), 1);
        assert_eq!(ts.nanos(), 500_000_000);
    }

    #[test]
    fn test_timestamp_conversions() {
        let ts = Timestamp::monotonic(2, 500_000_000);
        assert_eq!(ts.as_nanos(), 2_500_000_000);
        assert_eq!(ts.as_millis(), 2500);
        assert_eq!(ts.as_micros(), 2_500_000);
    }

    #[test]
    fn test_timestamp_is_zero() {
        assert!(Timestamp::ZERO_MONOTONIC.is_zero());
        assert!(Timestamp::ZERO_REALTIME.is_zero());
        assert!(!Timestamp::monotonic(1, 0).is_zero());
        assert!(!Timestamp::monotonic(0, 1).is_zero());
    }

    #[test]
    fn test_timestamp_add() {
        let ts = Timestamp::monotonic(10, 500_000_000);
        let dur = Duration::new(5, 300_000_000);

        let result = ts.checked_add(dur).unwrap();
        assert_eq!(result.seconds(), 15);
        assert_eq!(result.nanos(), 800_000_000);
    }

    #[test]
    fn test_timestamp_add_overflow_nanos() {
        let ts = Timestamp::monotonic(10, 700_000_000);
        let dur = Duration::new(5, 500_000_000);

        let result = ts.checked_add(dur).unwrap();
        assert_eq!(result.seconds(), 16);
        assert_eq!(result.nanos(), 200_000_000);
    }

    #[test]
    fn test_timestamp_sub() {
        let ts = Timestamp::monotonic(10, 800_000_000);
        let dur = Duration::new(5, 300_000_000);

        let result = ts.checked_sub(dur).unwrap();
        assert_eq!(result.seconds(), 5);
        assert_eq!(result.nanos(), 500_000_000);
    }

    #[test]
    fn test_timestamp_sub_borrow() {
        let ts = Timestamp::monotonic(10, 200_000_000);
        let dur = Duration::new(5, 500_000_000);

        let result = ts.checked_sub(dur).unwrap();
        assert_eq!(result.seconds(), 4);
        assert_eq!(result.nanos(), 700_000_000);
    }

    #[test]
    fn test_timestamp_duration_since() {
        let later = Timestamp::monotonic(15, 800_000_000);
        let earlier = Timestamp::monotonic(10, 300_000_000);

        let duration = later.duration_since(earlier).unwrap();
        assert_eq!(duration.seconds(), 5);
        assert_eq!(duration.nanos(), 500_000_000);
    }

    #[test]
    fn test_timestamp_duration_since_kind_mismatch() {
        let mono = Timestamp::monotonic(15, 0);
        let real = Timestamp::realtime(10, 0);

        assert!(mono.duration_since(real).is_none());
    }

    #[test]
    fn test_timestamp_ordering() {
        let ts1 = Timestamp::monotonic(10, 100);
        let ts2 = Timestamp::monotonic(10, 200);
        let ts3 = Timestamp::monotonic(11, 0);

        assert!(ts1 < ts2);
        assert!(ts2 < ts3);
        assert!(ts1 < ts3);
    }
}
