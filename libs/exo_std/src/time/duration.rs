//! Extensions pour Duration

use core::time::Duration;

/// Trait d'extension pour Duration
pub trait DurationExt {
    /// Convertit en secondes f64
    fn as_secs_f64(&self) -> f64;

    /// Convertit en secondes f32
    fn as_secs_f32(&self) -> f32;

    /// Retourne true si la durée est zéro
    fn is_zero(&self) -> bool;

    /// Multiplie par un facteur
    fn mul_f64(self, rhs: f64) -> Duration;

    /// Multiplie par un facteur
    fn mul_f32(self, rhs: f32) -> Duration;

    /// Divise par un facteur
    fn div_f64(self, rhs: f64) -> Duration;

    /// Divise par un facteur
    fn div_f32(self, rhs: f32) -> Duration;
}

impl DurationExt for Duration {
    fn as_secs_f64(&self) -> f64 {
        self.as_secs() as f64 + (self.subsec_nanos() as f64 / 1_000_000_000.0)
    }

    fn as_secs_f32(&self) -> f32 {
        self.as_secs() as f32 + (self.subsec_nanos() as f32 / 1_000_000_000.0)
    }

    fn is_zero(&self) -> bool {
        self.as_nanos() == 0
    }

    fn mul_f64(self, rhs: f64) -> Duration {
        let nanos = (self.as_nanos() as f64 * rhs) as u128;
        Duration::from_nanos(nanos as u64)
    }

    fn mul_f32(self, rhs: f32) -> Duration {
        let nanos = (self.as_nanos() as f32 * rhs) as u128;
        Duration::from_nanos(nanos as u64)
    }

    fn div_f64(self, rhs: f64) -> Duration {
        let nanos = (self.as_nanos() as f64 / rhs) as u128;
        Duration::from_nanos(nanos as u64)
    }

    fn div_f32(self, rhs: f32) -> Duration {
        let nanos = (self.as_nanos() as f32 / rhs) as u128;
        Duration::from_nanos(nanos as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_as_secs_f64() {
        let dur = Duration::from_millis(1500);
        assert_eq!(dur.as_secs_f64(), 1.5);

        let dur = Duration::from_secs(2);
        assert_eq!(dur.as_secs_f64(), 2.0);
    }

    #[test]
    fn test_is_zero() {
        assert!(Duration::ZERO.is_zero());
        assert!(!Duration::from_secs(1).is_zero());
    }

    #[test]
    fn test_mul_div() {
        let dur = Duration::from_secs(10);
        
        let doubled = dur.mul_f64(2.0);
        assert_eq!(doubled.as_secs(), 20);

        let halved = dur.div_f64(2.0);
        assert_eq!(halved.as_secs(), 5);
    }
}
