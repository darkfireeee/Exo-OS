// libs/exo_std/src/time.rs
//! Primitives temporelles pour mesurer et gérer le temps
//!
//! Ce module fournit des types pour mesurer le temps de manière précise et portable.

use core::time::Duration;
use core::ops::{Add, Sub, AddAssign, SubAssign};
use crate::syscall::time as sys;

/// Instant dans le temps à partir d'une référence monotone
///
/// Les Instants sont toujours croissants et ne reculent jamais, même si
/// l'horloge système est ajustée. Utilisez pour mesurer des durées.
///
/// # Exemple
/// ```no_run
/// use exo_std::time::Instant;
/// use core::time::Duration;
///
/// let start = Instant::now();
/// // ... opération chronométrée ...
/// let elapsed = start.elapsed();
/// println!("Temps écoulé: {:?}", elapsed);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(u64);

impl Instant {
    /// Retourne l'instant actuel
    ///
    /// # Exemple
    /// ```no_run
    /// use exo_std::time::Instant;
    ///
    /// let now = Instant::now();
    /// ```
    #[inline]
    pub fn now() -> Self {
        Instant(sys::get_time(sys::ClockType::Monotonic))
    }
    
    /// Calcule la durée écoulée depuis cet instant
    ///
    /// # Panics
    /// Panique si cet instant est dans le futur (ne devrait jamais arriver
    /// avec une horloge monotone).
    ///
    /// # Exemple
    /// ```no_run
    /// use exo_std::time::Instant;
    /// use exo_std::thread;
    /// use core::time::Duration;
    ///
    /// let start = Instant::now();
    /// thread::sleep(Duration::from_secs(1));
    /// let elapsed = start.elapsed();
    /// assert!(elapsed >= Duration::from_secs(1));
    /// ```
    #[inline]
    pub fn elapsed(&self) -> Duration {
        let now = Self::now();
        now.duration_since(*self)
    }
    
    /// Calcule la durée entre deux instants
    ///
    /// # Panics
    /// Panique si `earlier` est plus récent que `self`.
    ///
    /// # Exemple
    /// ```no_run
    /// use exo_std::time::Instant;
    /// use exo_std::thread;
    /// use core::time::Duration;
    ///
    /// let earlier = Instant::now();
    /// thread::sleep(Duration::from_millis(100));
    /// let later = Instant::now();
    ///
    /// let duration = later.duration_since(earlier);
    /// assert!(duration >= Duration::from_millis(100));
    /// ```
    #[inline]
    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.checked_duration_since(earlier)
            .expect("later instant is smaller than earlier")
    }
    
    /// Durée entre instants, retourne None si négatif
    #[inline]
    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
        if self.0 >= earlier.0 {
            Some(Duration::from_nanos(self.0 - earlier.0))
        } else {
            None
        }
    }
    
    /// Durée depuis earlier, saturée à 0 si earlier > self
    #[inline]
    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.checked_duration_since(earlier).unwrap_or(Duration::ZERO)
    }
    
    /// Ajoute une durée à cet instant
    ///
    /// # Panics
    /// Panique en cas de overflow.
    #[inline]
    pub fn checked_add(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_add(duration.as_nanos() as u64)
            .map(Instant)
    }
    
    /// Soustrait une durée de cet instant
    ///
    /// # Panics
    /// Panique en cas d'underflow.
    #[inline]
    pub fn checked_sub(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_sub(duration.as_nanos() as u64)
            .map(Instant)
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    
    #[inline]
    fn add(self, other: Duration) -> Instant {
        self.checked_add(other)
            .expect("overflow when adding duration to instant")
    }
}

impl AddAssign<Duration> for Instant {
    #[inline]
    fn add_assign(&mut self, other: Duration) {
        *self = *self + other;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;
    
    #[inline]
    fn sub(self, other: Duration) -> Instant {
        self.checked_sub(other)
            .expect("underflow when subtracting duration from instant")
    }
}

impl SubAssign<Duration> for Instant {
    #[inline]
    fn sub_assign(&mut self, other: Duration) {
        *self = *self - other;
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;
    
    #[inline]
    fn sub(self, other: Instant) -> Duration {
        self.duration_since(other)
    }
}

/// Endort le thread actuel pour la durée spécifiée
///
/// Cette fonction endort au moins pour la durée donnée, mais peut être plus longue
/// en fonction du scheduling du système.
///
/// # Exemple
/// ```no_run
/// use exo_std::time::sleep;
/// use core::time::Duration;
///
/// sleep(Duration::from_secs(1));
/// ```
#[inline]
pub fn sleep(duration: Duration) {
    sys::sleep_nanos(duration.as_nanos() as u64);
}

/// Extensions utilitaires pour Duration
pub trait DurationExt {
    /// Vérifie si la durée est zéro
    fn is_zero(&self) -> bool;
    
    /// Retourne la durée en microsecondes
    fn as_micros_f64(&self) -> f64;
    
    /// Retourne la durée en millisecondes f64
    fn as_millis_f64(&self) -> f64;
    
    /// Retourne la durée en secondes f64
    fn as_secs_f64(&self) -> f64;
}

impl DurationExt for Duration {
    #[inline]
    fn is_zero(&self) -> bool {
        self.as_nanos() == 0
    }
    
    #[inline]
    fn as_micros_f64(&self) -> f64 {
        self.as_nanos() as f64 / 1_000.0
    }
    
    #[inline]
    fn as_millis_f64(&self) -> f64 {
        self.as_nanos() as f64 / 1_000_000.0
    }
    
    #[inline]
    fn as_secs_f64(&self) -> f64 {
        self.as_nanos() as f64 / 1_000_000_000.0
    }
}

/// Chronomètre pour mesurer facilement des durées
///
/// # Exemple
/// ```no_run
/// use exo_std::time::Stopwatch;
///
/// let mut sw = Stopwatch::start();
/// // ... opération ...
/// let elapsed = sw.elapsed();
/// println!("Temps: {:?}", elapsed);
///
/// sw.reset();
/// // ... nouvelle mesure ...
/// ```
#[derive(Debug, Clone)]
pub struct Stopwatch {
    start: Instant,
}

impl Stopwatch {
    /// Crée et démarre un nouveau chronomètre
    #[inline]
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }
    
    /// Réinitialise le chronomètre
    #[inline]
    pub fn reset(&mut self) {
        self.start = Instant::now();
    }
    
    /// Retourne le temps écoulé
    #[inline]
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
    
    /// Réinitialise et retourne le temps écoulé avant reset
    #[inline]
    pub fn lap(&mut self) -> Duration {
        let elapsed = self.elapsed();
        self.reset();
        elapsed
    }
}

impl Default for Stopwatch {
    #[inline]
    fn default() -> Self {
        Self::start()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_instant_ordering() {
        let earlier = Instant::now();
        sleep(Duration::from_millis(1));
        let later = Instant::now();
        
        assert!(later > earlier);
    }
    
    #[test]
    fn test_instant_duration() {
        let start = Instant::now();
        sleep(Duration::from_millis(10));
        let elapsed = start.elapsed();
        
        // Au moins 10ms, mais peut être plus selon scheduling
        assert!(elapsed >= Duration::from_millis(10));
    }
    
    #[test]
    fn test_instant_arithmetic() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        
        assert!(later > now);
        assert_eq!(later - now, Duration::from_secs(1));
    }
    
    #[test]
    fn test_stopwatch() {
        let mut sw = Stopwatch::start();
        sleep(Duration::from_millis(10));
        
        let elapsed = sw.elapsed();
        assert!(elapsed >= Duration::from_millis(10));
        
        sw.reset();
        let new_elapsed = sw.elapsed();
        assert!(new_elapsed < elapsed);
    }
    
    #[test]
    fn test_duration_ext() {
        let dur = Duration::from_millis(1500);
        
        assert_eq!(dur.as_secs_f64(), 1.5);
        assert_eq!(dur.as_millis_f64(), 1500.0);
        assert!(!dur.is_zero());
        
        assert!(Duration::ZERO.is_zero());
    }
}
