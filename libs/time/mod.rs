// libs/exo_std/src/time/mod.rs
pub mod instant;
pub mod duration;
pub mod sleep;

pub use instant::Instant;
pub use duration::Duration;
pub use sleep::sleep;

use core::time::Duration as CoreDuration;

/// Convertit des secondes en Duration
pub fn seconds(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

/// Convertit des millisecondes en Duration
pub fn milliseconds(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// Convertit des microsecondes en Duration
pub fn microseconds(micros: u64) -> Duration {
    Duration::from_micros(micros)
}

/// Convertit des nanosecondes en Duration
pub fn nanoseconds(nanos: u64) -> Duration {
    Duration::from_nanos(nanos)
}

/// Attend pendant la durée spécifiée
pub fn sleep(duration: Duration) {
    match duration {
        Duration::Nanos(n) => sys_sleep_nanos(n),
        Duration::Micros(u) => sys_sleep_micros(u),
        Duration::Millis(m) => sys_sleep_millis(m),
        Duration::Seconds(s) => sys_sleep_seconds(s),
        Duration::Minutes(m) => sys_sleep_seconds(m * 60),
        Duration::Hours(h) => sys_sleep_seconds(h * 3600),
    }
}

// Appels système pour le sommeil
fn sys_sleep_nanos(nanos: u64) {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, ne rien faire
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep_nanos(nanos: u64);
            }
            sys_sleep_nanos(nanos);
        }
    }
}

fn sys_sleep_micros(micros: u64) {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, ne rien faire
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep_micros(micros: u64);
            }
            sys_sleep_micros(micros);
        }
    }
}

fn sys_sleep_millis(millis: u64) {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, ne rien faire
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep_millis(millis: u64);
            }
            sys_sleep_millis(millis);
        }
    }
}

fn sys_sleep_seconds(secs: u64) {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, ne rien faire
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep_seconds(secs: u64);
            }
            sys_sleep_seconds(secs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_duration_creation() {
        assert_eq!(seconds(1).as_nanos(), 1_000_000_000);
        assert_eq!(milliseconds(500).as_nanos(), 500_000_000);
        assert_eq!(microseconds(250).as_nanos(), 250_000);
        assert_eq!(nanoseconds(100).as_nanos(), 100);
    }
    
    #[test]
    fn test_duration_addition() {
        let d1 = seconds(1);
        let d2 = milliseconds(500);
        let d3 = d1 + d2;
        
        assert_eq!(d3.as_nanos(), 1_500_000_000);
    }
    
    #[test]
    fn test_duration_comparison() {
        assert!(seconds(1) > milliseconds(500));
        assert!(milliseconds(500) > microseconds(500));
        assert!(microseconds(500) > nanoseconds(500));
        assert_eq!(seconds(1), milliseconds(1000));
    }
}// libs/exo_std/src/time/mod.rs
pub mod instant;
pub mod duration;
pub mod sleep;

pub use instant::Instant;
pub use duration::Duration;
pub use sleep::sleep;

use core::time::Duration as CoreDuration;

/// Convertit des secondes en Duration
pub fn seconds(secs: u64) -> Duration {
    Duration::from_secs(secs)
}

/// Convertit des millisecondes en Duration
pub fn milliseconds(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// Convertit des microsecondes en Duration
pub fn microseconds(micros: u64) -> Duration {
    Duration::from_micros(micros)
}

/// Convertit des nanosecondes en Duration
pub fn nanoseconds(nanos: u64) -> Duration {
    Duration::from_nanos(nanos)
}

/// Attend pendant la durée spécifiée
pub fn sleep(duration: Duration) {
    match duration {
        Duration::Nanos(n) => sys_sleep_nanos(n),
        Duration::Micros(u) => sys_sleep_micros(u),
        Duration::Millis(m) => sys_sleep_millis(m),
        Duration::Seconds(s) => sys_sleep_seconds(s),
        Duration::Minutes(m) => sys_sleep_seconds(m * 60),
        Duration::Hours(h) => sys_sleep_seconds(h * 3600),
    }
}

// Appels système pour le sommeil
fn sys_sleep_nanos(nanos: u64) {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, ne rien faire
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep_nanos(nanos: u64);
            }
            sys_sleep_nanos(nanos);
        }
    }
}

fn sys_sleep_micros(micros: u64) {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, ne rien faire
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep_micros(micros: u64);
            }
            sys_sleep_micros(micros);
        }
    }
}

fn sys_sleep_millis(millis: u64) {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, ne rien faire
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep_millis(millis: u64);
            }
            sys_sleep_millis(millis);
        }
    }
}

fn sys_sleep_seconds(secs: u64) {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, ne rien faire
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_sleep_seconds(secs: u64);
            }
            sys_sleep_seconds(secs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_duration_creation() {
        assert_eq!(seconds(1).as_nanos(), 1_000_000_000);
        assert_eq!(milliseconds(500).as_nanos(), 500_000_000);
        assert_eq!(microseconds(250).as_nanos(), 250_000);
        assert_eq!(nanoseconds(100).as_nanos(), 100);
    }
    
    #[test]
    fn test_duration_addition() {
        let d1 = seconds(1);
        let d2 = milliseconds(500);
        let d3 = d1 + d2;
        
        assert_eq!(d3.as_nanos(), 1_500_000_000);
    }
    
    #[test]
    fn test_duration_comparison() {
        assert!(seconds(1) > milliseconds(500));
        assert!(milliseconds(500) > microseconds(500));
        assert!(microseconds(500) > nanoseconds(500));
        assert_eq!(seconds(1), milliseconds(1000));
    }
}