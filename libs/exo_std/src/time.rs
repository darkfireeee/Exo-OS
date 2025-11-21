// libs/exo_std/src/time.rs
use core::time::Duration;

/// Représente un instant dans le temps
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(u64);

impl Instant {
    /// Retourne l'instant actuel
    pub fn now() -> Self {
        // Dans un vrai OS, appel système pour obtenir le temps
        // Pour l'instant, on retourne 0 ou une valeur simulée
        #[cfg(feature = "test_mode")]
        {
            Instant(0)
        }

        #[cfg(not(feature = "test_mode"))]
        {
            unsafe {
                extern "C" {
                    fn sys_time_now() -> u64;
                }
                Instant(sys_time_now())
            }
        }
    }

    /// Calcule la durée écoulée depuis cet instant
    pub fn elapsed(&self) -> Duration {
        let now = Self::now();
        if now.0 >= self.0 {
            Duration::from_nanos(now.0 - self.0)
        } else {
            Duration::from_nanos(0)
        }
    }
}

/// Endort le thread actuel pour une durée donnée
pub fn sleep(duration: Duration) {
    #[cfg(feature = "test_mode")]
    {
        // Simulation
    }

    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep(nanos: u64);
            }
            sys_sleep(duration.as_nanos() as u64);
        }
    }
}
