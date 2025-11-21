// libs/exo_std/src/time/instant.rs
use core::ops::{Add, Sub};

/// Représente un instant précis dans le temps
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant {
    tsc: u64, // Timestamp Counter
    epoch: u64, // Époque Unix en nanosecondes
}

impl Instant {
    /// Retourne l'instant actuel
    pub fn now() -> Self {
        sys_get_instant()
    }
    
    /// Retourne la durée écoulée depuis cet instant
    pub fn elapsed(&self) -> super::Duration {
        let now = Self::now();
        now - *self
    }
    
    /// Ajoute une durée à cet instant
    pub fn add(&self, duration: super::Duration) -> Self {
        *self + duration
    }
    
    /// Soustrait une durée à cet instant
    pub fn sub(&self, duration: super::Duration) -> Self {
        *self - duration
    }
    
    /// Convertit en nanosecondes depuis l'époque Unix
    pub fn as_unix_nanos(&self) -> u128 {
        (self.epoch as u128) << 32 | (self.tsc as u128)
    }
}

impl Add<super::Duration> for Instant {
    type Output = Self;
    
    fn add(self, rhs: super::Duration) -> Self::Output {
        let nanos = rhs.as_nanos();
        let new_tsc = self.tsc.wrapping_add(nanos as u64);
        let new_epoch = if nanos > u64::MAX as u128 {
            self.epoch + (nanos >> 64) as u64
        } else {
            self.epoch
        };
        
        Instant {
            tsc: new_tsc,
            epoch: new_epoch,
        }
    }
}

impl Sub<super::Duration> for Instant {
    type Output = Self;
    
    fn sub(self, rhs: super::Duration) -> Self::Output {
        let nanos = rhs.as_nanos();
        let new_tsc = self.tsc.wrapping_sub(nanos as u64);
        let new_epoch = if nanos > u64::MAX as u128 {
            self.epoch - (nanos >> 64) as u64
        } else {
            self.epoch
        };
        
        Instant {
            tsc: new_tsc,
            epoch: new_epoch,
        }
    }
}

impl Sub<Instant> for Instant {
    type Output = super::Duration;
    
    fn sub(self, rhs: Instant) -> Self::Output {
        let tsc_diff = if self.tsc >= rhs.tsc {
            self.tsc - rhs.tsc
        } else {
            rhs.tsc - self.tsc
        };
        
        let epoch_diff = if self.epoch >= rhs.epoch {
            (self.epoch - rhs.epoch) as u128 * 1_000_000_000
        } else {
            (rhs.epoch - self.epoch) as u128 * 1_000_000_000
        };
        
        super::Duration::from_nanos(epoch_diff + tsc_diff as u128)
    }
}

// Appel système pour obtenir l'instant actuel
fn sys_get_instant() -> Instant {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, utiliser un compteur monotone
        static COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let count = COUNTER.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        
        Instant {
            tsc: count,
            epoch: 0,
        }
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_get_tsc() -> u64;
                fn sys_get_unix_time_nanos() -> u64;
            }
            
            Instant {
                tsc: sys_get_tsc(),
                epoch: sys_get_unix_time_nanos(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{Duration, seconds};
    
    #[test]
    fn test_instant_now() {
        let now = Instant::now();
        assert!(now.tsc > 0);
    }
    
    #[test]
    fn test_instant_elapsed() {
        let start = Instant::now();
        crate::thread::sleep(seconds(1));
        let elapsed = start.elapsed();
        
        assert!(elapsed >= seconds(1));
        assert!(elapsed <= seconds(2));
    }
    
    #[test]
    fn test_instant_arithmetic() {
        let now = Instant::now();
        let future = now + seconds(1);
        let past = now - seconds(1);
        
        assert!(future > now);
        assert!(past < now);
        
        let duration = future - now;
        assert!(duration >= seconds(1));
        assert!(duration <= seconds(1) + milliseconds(10));
        
        let duration = now - past;
        assert!(duration >= seconds(1));
        assert!(duration <= seconds(1) + milliseconds(10));
    }
    
    #[test]
    fn test_instant_unix() {
        let now = Instant::now();
        let unix_nanos = now.as_unix_nanos();
        
        assert!(unix_nanos > 0);
        assert!(unix_nanos < 2_000_000_000_000_000_000); // ~2033
    }
}