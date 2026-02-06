//! Type Instant pour mesurer le temps monotone

use core::ops::{Add, Sub, AddAssign, SubAssign};
use core::time::Duration;

/// Instant dans le temps (monotone)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    nanos: u64,
}

impl Instant {
    /// Crée un Instant à partir de nanosecondes
    const fn from_nanos(nanos: u64) -> Self {
        Self { nanos }
    }

    /// Retourne l'instant actuel
    pub fn now() -> Self {
        #[cfg(feature = "test_mode")]
        {
            Self::from_nanos(0)
        }
        
        #[cfg(not(feature = "test_mode"))]
        unsafe {
            use crate::syscall::time::{get_time, ClockType};
            let nanos = get_time(ClockType::Monotonic);
            Self::from_nanos(nanos)
        }
    }

    /// Retourne le temps écoulé depuis cet instant
    pub fn elapsed(&self) -> Duration {
        Self::now() - *self
    }

    /// Retourne la durée depuis un autre instant
    pub fn duration_since(&self, earlier: Instant) -> Duration {
        *self - earlier
    }

    /// Checked addition
    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.nanos
            .checked_add(duration.as_nanos() as u64)
            .map(Self::from_nanos)
    }

    /// Checked subtraction
    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.nanos
            .checked_sub(duration.as_nanos() as u64)
            .map(Self::from_nanos)
    }

    /// Saturating addition
    pub fn saturating_add(&self, duration: Duration) -> Self {
        Self::from_nanos(self.nanos.saturating_add(duration.as_nanos() as u64))
    }

    /// Saturating subtraction
    pub fn saturating_sub(&self, duration: Duration) -> Self {
        Self::from_nanos(self.nanos.saturating_sub(duration.as_nanos() as u64))
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;

    fn add(self, other: Duration) -> Instant {
        self.checked_add(other)
            .expect("overflow when adding duration to instant")
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, other: Duration) {
        *self = *self + other;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;

    fn sub(self, other: Duration) -> Instant {
        self.checked_sub(other)
            .expect("overflow when subtracting duration from instant")
    }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, other: Duration) {
        *self = *self - other;
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;

    fn sub(self, other: Instant) -> Duration {
        Duration::from_nanos(self.nanos.saturating_sub(other.nanos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instant_arithmetic() {
        let t1 = Instant::now();
        let t2 = t1 + Duration::from_secs(5);
        let t3 = t2 - Duration::from_secs(2);

        let diff = t2 - t1;
        assert_eq!(diff.as_secs(), 5);

        let diff2 = t2 - t3;
        assert_eq!(diff2.as_secs(), 2);
    }

    #[test]
    fn test_instant_checked() {
        let t = Instant::now();
        
        assert!(t.checked_add(Duration::from_secs(100)).is_some());
        assert!(t.checked_sub(Duration::from_secs(100)).is_some());
    }

    #[test]
    fn test_instant_saturating() {
        let t = Instant::from_nanos(100);
        
        let t2 = t.saturating_add(Duration::from_nanos(50));
        assert_eq!(t2.nanos, 150);

        let t3 = t.saturating_sub(Duration::from_nanos(150));
        assert_eq!(t3.nanos, 0);
    }
}
