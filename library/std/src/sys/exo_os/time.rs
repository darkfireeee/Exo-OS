//! `std::sys::exo_os::time` — `Instant` and `SystemTime` over `SYS_CLOCK_GETTIME = 228`.

pub use core::time::Duration;
use core::ops::{Add, Sub};
use exo_libc::{ExoOs, Pal};
use exo_syscall::process::{CLOCK_MONOTONIC, CLOCK_REALTIME};

/// Monotonic clock measurement (`std::time::Instant`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    nanos: u64,
}

impl Instant {
    #[inline]
    pub fn now() -> Self {
        let ts = ExoOs::clock_gettime(CLOCK_MONOTONIC).unwrap_or_default();
        let nanos = (ts.tv_sec.max(0) as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(ts.tv_nsec.max(0) as u64);
        Self { nanos }
    }

    #[inline]
    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.checked_duration_since(earlier)
            .unwrap_or(Duration::ZERO)
    }

    #[inline]
    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
        self.nanos
            .checked_sub(earlier.nanos)
            .map(Duration::from_nanos)
    }

    #[inline]
    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.duration_since(earlier)
    }

    #[inline]
    pub fn elapsed(&self) -> Duration {
        Self::now().duration_since(*self)
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    #[inline]
    fn add(self, rhs: Duration) -> Instant {
        Instant {
            nanos: self.nanos.saturating_add(rhs.as_nanos() as u64),
        }
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;
    #[inline]
    fn sub(self, rhs: Instant) -> Duration {
        self.duration_since(rhs)
    }
}

/// Wall-clock measurement (`std::time::SystemTime`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SystemTime {
    nanos: u64,
}

/// `1970-01-01 00:00:00 UTC`.
pub const UNIX_EPOCH: SystemTime = SystemTime { nanos: 0 };

impl SystemTime {
    #[inline]
    pub fn now() -> Self {
        let ts = ExoOs::clock_gettime(CLOCK_REALTIME).unwrap_or_default();
        let nanos = (ts.tv_sec.max(0) as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(ts.tv_nsec.max(0) as u64);
        Self { nanos }
    }

    #[inline]
    pub fn duration_since(&self, earlier: SystemTime) -> Result<Duration, Duration> {
        if self.nanos >= earlier.nanos {
            Ok(Duration::from_nanos(self.nanos - earlier.nanos))
        } else {
            Err(Duration::from_nanos(earlier.nanos - self.nanos))
        }
    }
}
