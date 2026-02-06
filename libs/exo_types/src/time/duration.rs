//! Duration type for time intervals
//!
//! High-precision duration with nanosecond resolution.

use core::fmt;
use core::ops::{Add, Sub, Mul, Div};

/// Duration type
///
/// Represents a time interval with nanosecond precision.
/// Can be positive or negative.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration {
    /// Seconds component (can be negative)
    seconds: i64,
    /// Nanoseconds component (0-999,999,999)
    nanos: u32,
}

impl Duration {
    /// Maximum nanoseconds value
    pub const MAX_NANOS: u32 = 999_999_999;

    /// Nanoseconds per second
    pub const NANOS_PER_SEC: u32 = 1_000_000_000;

    /// Nanoseconds per millisecond
    pub const NANOS_PER_MILLI: u32 = 1_000_000;

    /// Nanoseconds per microsecond
    pub const NANOS_PER_MICRO: u32 = 1_000;

    /// Zero duration
    pub const ZERO: Self = Self {
        seconds: 0,
        nanos: 0,
    };

    /// One second
    pub const SECOND: Self = Self {
        seconds: 1,
        nanos: 0,
    };

    /// One millisecond
    pub const MILLISECOND: Self = Self {
        seconds: 0,
        nanos: Self::NANOS_PER_MILLI,
    };

    /// One microsecond
    pub const MICROSECOND: Self = Self {
        seconds: 0,
        nanos: Self::NANOS_PER_MICRO,
    };

    /// One nanosecond
    pub const NANOSECOND: Self = Self {
        seconds: 0,
        nanos: 1,
    };

    /// Create new duration with validation
    ///
    /// # Panics
    /// Panics in debug mode if nanos >= 1_000_000_000
    #[inline(always)]
    pub const fn new(seconds: i64, nanos: u32) -> Self {
        debug_assert!(nanos < Self::NANOS_PER_SEC, "Nanoseconds must be < 1_000_000_000");
        Self { seconds, nanos }
    }

    /// Try to create duration, returning None if invalid
    #[inline]
    pub const fn try_new(seconds: i64, nanos: u32) -> Option<Self> {
        if nanos < Self::NANOS_PER_SEC {
            Some(Self { seconds, nanos })
        } else {
            None
        }
    }

    /// Create from total nanoseconds
    #[inline]
    pub const fn from_nanos(nanos: i64) -> Self {
        let seconds = nanos / (Self::NANOS_PER_SEC as i64);
        let remaining_nanos = (nanos % (Self::NANOS_PER_SEC as i64)).abs() as u32;
        Self {
            seconds,
            nanos: remaining_nanos,
        }
    }

    /// Create from milliseconds
    #[inline]
    pub const fn from_millis(millis: i64) -> Self {
        let seconds = millis / 1000;
        let nanos = ((millis % 1000).abs() as u32) * Self::NANOS_PER_MILLI;
        Self { seconds, nanos }
    }

    /// Create from microseconds
    #[inline]
    pub const fn from_micros(micros: i64) -> Self {
        let seconds = micros / 1_000_000;
        let nanos = ((micros % 1_000_000).abs() as u32) * Self::NANOS_PER_MICRO;
        Self { seconds, nanos }
    }

    /// Create from seconds (as f64)
    #[inline]
    pub fn from_secs_f64(secs: f64) -> Self {
        let seconds = secs as i64;
        let fract = secs - (seconds as f64);
        let nanos = ((fract * 1_000_000_000.0).abs() as u32).min(Self::MAX_NANOS);
        Self { seconds, nanos }
    }

    /// Get seconds component
    #[inline(always)]
    pub const fn seconds(self) -> i64 {
        self.seconds
    }

    /// Get nanoseconds component
    #[inline(always)]
    pub const fn nanos(self) -> u32 {
        self.nanos
    }

    /// Convert to total nanoseconds (may overflow for large durations)
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
            .saturating_add((self.nanos / Self::NANOS_PER_MILLI) as i64)
    }

    /// Convert to microseconds
    #[inline]
    pub const fn as_micros(self) -> i64 {
        self.seconds
            .saturating_mul(1_000_000)
            .saturating_add((self.nanos / Self::NANOS_PER_MICRO) as i64)
    }

    /// Convert to seconds as f64
    #[inline]
    pub fn as_secs_f64(self) -> f64 {
        self.seconds as f64 + (self.nanos as f64 / 1_000_000_000.0)
    }

    /// Check if duration is zero
    #[inline(always)]
    pub const fn is_zero(self) -> bool {
        self.seconds == 0 && self.nanos == 0
    }

    /// Check if duration is positive
    #[inline(always)]
    pub const fn is_positive(self) -> bool {
        self.seconds > 0 || (self.seconds == 0 && self.nanos > 0)
    }

    /// Check if duration is negative
    #[inline(always)]
    pub const fn is_negative(self) -> bool {
        self.seconds < 0
    }

    /// Get absolute value
    #[inline]
    pub const fn abs(self) -> Self {
        if self.is_negative() {
            Self {
                seconds: -self.seconds,
                nanos: self.nanos,
            }
        } else {
            self
        }
    }

    /// Checked addition
    #[inline]
    pub const fn checked_add(self, other: Duration) -> Option<Self> {
        let total_nanos = (self.nanos as i64).wrapping_add(other.nanos as i64);
        let extra_secs = total_nanos / (Self::NANOS_PER_SEC as i64);
        let new_nanos = (total_nanos % (Self::NANOS_PER_SEC as i64)).abs() as u32;

        match self.seconds.checked_add(other.seconds) {
            Some(s) => match s.checked_add(extra_secs) {
                Some(secs) => Some(Self {
                    seconds: secs,
                    nanos: new_nanos,
                }),
                None => None,
            },
            None => None,
        }
    }

    /// Checked subtraction
    #[inline]
    pub const fn checked_sub(self, other: Duration) -> Option<Self> {
        let total_nanos = (self.nanos as i64).wrapping_sub(other.nanos as i64);
        let (extra_secs, new_nanos) = if total_nanos < 0 {
            (-1, (total_nanos + (Self::NANOS_PER_SEC as i64)) as u32)
        } else {
            (0, total_nanos as u32)
        };

        match self.seconds.checked_sub(other.seconds) {
            Some(s) => match s.checked_add(extra_secs) {
                Some(secs) => Some(Self {
                    seconds: secs,
                    nanos: new_nanos,
                }),
                None => None,
            },
            None => None,
        }
    }

    /// Checked multiplication by scalar
    #[inline]
    pub const fn checked_mul(self, scalar: i64) -> Option<Self> {
        let total_nanos = (self.nanos as i64).wrapping_mul(scalar);
        let nanos_secs = total_nanos / (Self::NANOS_PER_SEC as i64);
        let new_nanos = (total_nanos % (Self::NANOS_PER_SEC as i64)).abs() as u32;

        match self.seconds.checked_mul(scalar) {
            Some(s) => match s.checked_add(nanos_secs) {
                Some(secs) => Some(Self {
                    seconds: secs,
                    nanos: new_nanos,
                }),
                None => None,
            },
            None => None,
        }
    }

    /// Checked division by scalar
    #[inline]
    pub const fn checked_div(self, scalar: i64) -> Option<Self> {
        if scalar == 0 {
            return None;
        }

        let total_nanos = self.as_nanos();
        Some(Self::from_nanos(total_nanos / scalar))
    }

    /// Saturating addition
    #[inline]
    pub fn saturating_add(self, other: Duration) -> Self {
        self.checked_add(other).unwrap_or_else(|| {
            if other.is_positive() {
                Self {
                    seconds: i64::MAX,
                    nanos: Self::MAX_NANOS,
                }
            } else {
                Self {
                    seconds: i64::MIN,
                    nanos: 0,
                }
            }
        })
    }

    /// Saturating subtraction
    #[inline]
    pub fn saturating_sub(self, other: Duration) -> Self {
        self.checked_sub(other).unwrap_or_else(|| {
            if other.is_positive() {
                Self {
                    seconds: i64::MIN,
                    nanos: 0,
                }
            } else {
                Self {
                    seconds: i64::MAX,
                    nanos: Self::MAX_NANOS,
                }
            }
        })
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Duration")
            .field("seconds", &self.seconds)
            .field("nanos", &self.nanos)
            .finish()
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_negative() {
            write!(f, "-{}.{:09}s", self.seconds.abs(), self.nanos)
        } else {
            write!(f, "{}.{:09}s", self.seconds, self.nanos)
        }
    }
}

impl Add for Duration {
    type Output = Self;

    #[inline]
    fn add(self, other: Self) -> Self {
        self.saturating_add(other)
    }
}

impl Sub for Duration {
    type Output = Self;

    #[inline]
    fn sub(self, other: Self) -> Self {
        self.saturating_sub(other)
    }
}

impl Mul<i64> for Duration {
    type Output = Self;

    #[inline]
    fn mul(self, scalar: i64) -> Self {
        self.checked_mul(scalar).unwrap_or_else(|| {
            if (self.is_positive() && scalar > 0) || (self.is_negative() && scalar < 0) {
                Self { seconds: i64::MAX, nanos: Self::MAX_NANOS }
            } else {
                Self { seconds: i64::MIN, nanos: 0 }
            }
        })
    }
}

impl Div<i64> for Duration {
    type Output = Self;

    #[inline]
    fn div(self, scalar: i64) -> Self {
        self.checked_div(scalar).expect("Division by zero")
    }
}

impl Default for Duration {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn test_duration_creation() {
        let dur = Duration::new(10, 500_000_000);
        assert_eq!(dur.seconds(), 10);
        assert_eq!(dur.nanos(), 500_000_000);
    }

    #[test]
    fn test_duration_try_new() {
        assert!(Duration::try_new(10, 500_000_000).is_some());
        assert!(Duration::try_new(10, 1_000_000_000).is_none());
    }

    #[test]
    fn test_duration_from_nanos() {
        let dur = Duration::from_nanos(1_500_000_000);
        assert_eq!(dur.seconds(), 1);
        assert_eq!(dur.nanos(), 500_000_000);
    }

    #[test]
    fn test_duration_from_millis() {
        let dur = Duration::from_millis(1500);
        assert_eq!(dur.seconds(), 1);
        assert_eq!(dur.nanos(), 500_000_000);
    }

    #[test]
    fn test_duration_from_micros() {
        let dur = Duration::from_micros(1_500_000);
        assert_eq!(dur.seconds(), 1);
        assert_eq!(dur.nanos(), 500_000_000);
    }

    #[test]
    fn test_duration_conversions() {
        let dur = Duration::new(2, 500_000_000);
        assert_eq!(dur.as_nanos(), 2_500_000_000);
        assert_eq!(dur.as_millis(), 2500);
        assert_eq!(dur.as_micros(), 2_500_000);
        assert!((dur.as_secs_f64() - 2.5).abs() < 0.0001);
    }

    #[test]
    fn test_duration_is_zero() {
        assert!(Duration::ZERO.is_zero());
        assert!(!Duration::SECOND.is_zero());
    }

    #[test]
    fn test_duration_is_positive() {
        assert!(Duration::SECOND.is_positive());
        assert!(!Duration::ZERO.is_positive());
        assert!(!Duration::new(-1, 0).is_positive());
    }

    #[test]
    fn test_duration_is_negative() {
        assert!(Duration::new(-1, 0).is_negative());
        assert!(!Duration::ZERO.is_negative());
        assert!(!Duration::SECOND.is_negative());
    }

    #[test]
    fn test_duration_abs() {
        let neg = Duration::new(-5, 300_000_000);
        let abs = neg.abs();
        assert_eq!(abs.seconds(), 5);
        assert_eq!(abs.nanos(), 300_000_000);
    }

    #[test]
    fn test_duration_add() {
        let dur1 = Duration::new(5, 500_000_000);
        let dur2 = Duration::new(3, 700_000_000);

        let result = dur1.checked_add(dur2).unwrap();
        assert_eq!(result.seconds(), 9);
        assert_eq!(result.nanos(), 200_000_000);
    }

    #[test]
    fn test_duration_sub() {
        let dur1 = Duration::new(10, 300_000_000);
        let dur2 = Duration::new(5, 800_000_000);

        let result = dur1.checked_sub(dur2).unwrap();
        assert_eq!(result.seconds(), 4);
        assert_eq!(result.nanos(), 500_000_000);
    }

    #[test]
    fn test_duration_mul() {
        let dur = Duration::new(2, 500_000_000);
        let result = dur.checked_mul(3).unwrap();

        assert_eq!(result.seconds(), 7);
        assert_eq!(result.nanos(), 500_000_000);
    }

    #[test]
    fn test_duration_div() {
        let dur = Duration::new(10, 0);
        let result = dur.checked_div(2).unwrap();

        assert_eq!(result.seconds(), 5);
        assert_eq!(result.nanos(), 0);
    }

    #[test]
    fn test_duration_constants() {
        assert_eq!(Duration::SECOND.as_nanos(), 1_000_000_000);
        assert_eq!(Duration::MILLISECOND.as_nanos(), 1_000_000);
        assert_eq!(Duration::MICROSECOND.as_nanos(), 1_000);
        assert_eq!(Duration::NANOSECOND.as_nanos(), 1);
    }
}
