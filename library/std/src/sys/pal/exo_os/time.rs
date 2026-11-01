//! `Timespec` helper for `std::sys::time` on Exo-OS.

use crate::cmp::Ordering;
use crate::hash::{Hash, Hasher};
use crate::time::Duration;
use super::syscall::RawTimespec;

const NSEC_PER_SEC: i64 = 1_000_000_000;

#[derive(Copy, Clone, Debug)]
pub struct Timespec {
    pub t: RawTimespec,
}

impl Timespec {
    pub const MAX: Timespec = Self::new(i64::MAX, 1_000_000_000 - 1);
    pub const MIN: Timespec = Self::new(i64::MIN, 0);

    pub const fn zero() -> Timespec {
        Timespec { t: RawTimespec { tv_sec: 0, tv_nsec: 0 } }
    }

    pub const fn new(tv_sec: i64, tv_nsec: i64) -> Timespec {
        assert!(tv_nsec >= 0 && tv_nsec < NSEC_PER_SEC);
        Timespec { t: RawTimespec { tv_sec, tv_nsec } }
    }

    pub fn sub_timespec(&self, other: &Timespec) -> Result<Duration, Duration> {
        fn sub_ge_to_unsigned(a: i64, b: i64) -> u64 {
            debug_assert!(a >= b);
            a.wrapping_sub(b).cast_unsigned()
        }

        if self >= other {
            Ok(if self.t.tv_nsec >= other.t.tv_nsec {
                Duration::new(
                    sub_ge_to_unsigned(self.t.tv_sec, other.t.tv_sec),
                    (self.t.tv_nsec - other.t.tv_nsec) as u32,
                )
            } else {
                Duration::new(
                    sub_ge_to_unsigned(self.t.tv_sec - 1, other.t.tv_sec),
                    (self.t.tv_nsec + NSEC_PER_SEC - other.t.tv_nsec) as u32,
                )
            })
        } else {
            match other.sub_timespec(self) {
                Ok(d) => Err(d),
                Err(d) => Ok(d),
            }
        }
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<Timespec> {
        let mut secs = self.t.tv_sec.checked_add_unsigned(other.as_secs())?;
        let mut nsec = (other.subsec_nanos() as i64) + self.t.tv_nsec;
        if nsec >= NSEC_PER_SEC {
            nsec -= NSEC_PER_SEC;
            secs = secs.checked_add(1)?;
        }
        Some(Timespec { t: RawTimespec { tv_sec: secs, tv_nsec: nsec } })
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<Timespec> {
        let mut secs = self.t.tv_sec.checked_sub_unsigned(other.as_secs())?;
        let mut nsec = self.t.tv_nsec - (other.subsec_nanos() as i64);
        if nsec < 0 {
            nsec += NSEC_PER_SEC;
            secs = secs.checked_sub(1)?;
        }
        Some(Timespec { t: RawTimespec { tv_sec: secs, tv_nsec: nsec } })
    }
}

impl PartialEq for Timespec {
    fn eq(&self, other: &Timespec) -> bool {
        self.t.tv_sec == other.t.tv_sec && self.t.tv_nsec == other.t.tv_nsec
    }
}

impl Eq for Timespec {}

impl PartialOrd for Timespec {
    fn partial_cmp(&self, other: &Timespec) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Timespec {
    fn cmp(&self, other: &Timespec) -> Ordering {
        (self.t.tv_sec, self.t.tv_nsec).cmp(&(other.t.tv_sec, other.t.tv_nsec))
    }
}

impl Hash for Timespec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.t.tv_sec.hash(state);
        self.t.tv_nsec.hash(state);
    }
}
