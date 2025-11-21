// libs/exo_std/src/time/duration.rs
use core::ops::{Add, Sub, Mul, Div};

/// Représente une durée de temps
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Duration {
    Nanos(u64),
    Micros(u64),
    Millis(u64),
    Seconds(u64),
    Minutes(u64),
    Hours(u64),
}

impl Duration {
    /// Crée une durée depuis des nanosecondes
    pub fn from_nanos(nanos: u64) -> Self {
        if nanos < 1_000 {
            Duration::Nanos(nanos)
        } else if nanos < 1_000_000 {
            Duration::Micros(nanos / 1_000)
        } else if nanos < 1_000_000_000 {
            Duration::Millis(nanos / 1_000_000)
        } else if nanos < 60_000_000_000 {
            Duration::Seconds(nanos / 1_000_000_000)
        } else if nanos < 3_600_000_000_000 {
            Duration::Minutes(nanos / 60_000_000_000)
        } else {
            Duration::Hours(nanos / 3_600_000_000_000)
        }
    }
    
    /// Crée une durée depuis des microsecondes
    pub fn from_micros(micros: u64) -> Self {
        Self::from_nanos(micros * 1_000)
    }
    
    /// Crée une durée depuis des millisecondes
    pub fn from_millis(millis: u64) -> Self {
        Self::from_nanos(millis * 1_000_000)
    }
    
    /// Crée une durée depuis des secondes
    pub fn from_secs(secs: u64) -> Self {
        Self::from_nanos(secs * 1_000_000_000)
    }
    
    /// Convertit en nanosecondes
    pub fn as_nanos(&self) -> u128 {
        match self {
            Duration::Nanos(n) => *n as u128,
            Duration::Micros(u) => (*u as u128) * 1_000,
            Duration::Millis(m) => (*m as u128) * 1_000_000,
            Duration::Seconds(s) => (*s as u128) * 1_000_000_000,
            Duration::Minutes(m) => (*m as u128) * 60_000_000_000,
            Duration::Hours(h) => (*h as u128) * 3_600_000_000_000,
        }
    }
    
    /// Convertit en microsecondes
    pub fn as_micros(&self) -> u128 {
        self.as_nanos() / 1_000
    }
    
    /// Convertit en millisecondes
    pub fn as_millis(&self) -> u128 {
        self.as_nanos() / 1_000_000
    }
    
    /// Convertit en secondes
    pub fn as_secs(&self) -> u128 {
        self.as_nanos() / 1_000_000_000
    }
}

impl Add for Duration {
    type Output = Self;
    
    fn add(self, rhs: Self) -> Self::Output {
        Duration::from_nanos((self.as_nanos() + rhs.as_nanos()) as u64)
    }
}

impl Sub for Duration {
    type Output = Self;
    
    fn sub(self, rhs: Self) -> Self::Output {
        let self_nanos = self.as_nanos();
        let rhs_nanos = rhs.as_nanos();
        
        if self_nanos < rhs_nanos {
            Duration::from_nanos(0)
        } else {
            Duration::from_nanos((self_nanos - rhs_nanos) as u64)
        }
    }
}

impl Mul<u32> for Duration {
    type Output = Self;
    
    fn mul(self, rhs: u32) -> Self::Output {
        Duration::from_nanos((self.as_nanos() * rhs as u128) as u64)
    }
}

impl Div<u32> for Duration {
    type Output = Self;
    
    fn div(self, rhs: u32) -> Self::Output {
        if rhs == 0 {
            Duration::from_nanos(0)
        } else {
            Duration::from_nanos((self.as_nanos() / rhs as u128) as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_duration_from() {
        assert_eq!(Duration::from_nanos(500), Duration::Nanos(500));
        assert_eq!(Duration::from_nanos(1500), Duration::Micros(1));
        assert_eq!(Duration::from_nanos(2500000), Duration::Millis(2));
        assert_eq!(Duration::from_nanos(3500000000), Duration::Seconds(3));
        assert_eq!(Duration::from_nanos(120000000000), Duration::Minutes(2));
        assert_eq!(Duration::from_nanos(7200000000000), Duration::Hours(2));
    }
    
    #[test]
    fn test_duration_as() {
        assert_eq!(Duration::Nanos(500).as_nanos(), 500);
        assert_eq!(Duration::Micros(1).as_nanos(), 1000);
        assert_eq!(Duration::Millis(2).as_nanos(), 2000000);
        assert_eq!(Duration::Seconds(3).as_nanos(), 3000000000);
        assert_eq!(Duration::Minutes(2).as_nanos(), 120000000000);
        assert_eq!(Duration::Hours(2).as_nanos(), 7200000000000);
    }
    
    #[test]
    fn test_duration_arithmetic() {
        let d1 = Duration::Seconds(1);
        let d2 = Duration::Millis(500);
        
        // Addition
        let d3 = d1 + d2;
        assert_eq!(d3.as_nanos(), 1_500_000_000);
        
        // Soustraction
        let d4 = d1 - d2;
        assert_eq!(d4.as_nanos(), 500_000_000);
        
        // Multiplication
        let d5 = d1 * 2;
        assert_eq!(d5.as_nanos(), 2_000_000_000);
        
        // Division
        let d6 = d1 / 2;
        assert_eq!(d6.as_nanos(), 500_000_000);
    }
    
    #[test]
    fn test_duration_comparison() {
        assert!(Duration::Seconds(1) > Duration::Millis(500));
        assert!(Duration::Millis(500) > Duration::Micros(500));
        assert!(Duration::Micros(500) > Duration::Nanos(500));
        assert_eq!(Duration::Seconds(1), Duration::Millis(1000));
    }
    
    #[test]
    fn test_duration_subtraction_underflow() {
        let d1 = Duration::Millis(500);
        let d2 = Duration::Seconds(1);
        
        let result = d1 - d2;
        assert_eq!(result, Duration::from_nanos(0));
    }
}