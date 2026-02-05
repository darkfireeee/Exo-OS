//! Gestion du temps
//!
//! Ce module fournit des outils pour mesurer et manipuler le temps.

pub mod instant;
pub mod duration;

// Réexportations
pub use instant::Instant;
pub use duration::DurationExt;

use crate::error::IoError;

/// Dort pendant une durée
pub fn sleep(dur: core::time::Duration) {
    #[cfg(not(feature = "test_mode"))]
    unsafe {
        crate::syscall::time::sleep_nanos(dur.as_nanos() as u64);
    }
    
    #[cfg(feature = "test_mode")]
    {
        let _ = dur;
    }
}

/// Stopwatch pour mesurer des durées
pub struct Stopwatch {
    start: Instant,
    last_lap: Instant,
}

impl Stopwatch {
    /// Démarre un nouveau stopwatch
    pub fn start() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last_lap: now,
        }
    }

    /// Retourne le temps total écoulé
    pub fn elapsed(&self) -> core::time::Duration {
        self.start.elapsed()
    }

    /// Enregistre un lap et retourne le temps depuis le dernier lap
    pub fn lap(&mut self) -> core::time::Duration {
        let now = Instant::now();
        let lap_time = now - self.last_lap;
        self.last_lap = now;
        lap_time
    }

    /// Reset le stopwatch
    pub fn reset(&mut self) {
        let now = Instant::now();
        self.start = now;
        self.last_lap = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stopwatch() {
        let mut sw = Stopwatch::start();
        let _lap1 = sw.lap();
        let _elapsed = sw.elapsed();
        sw.reset();
    }
}
