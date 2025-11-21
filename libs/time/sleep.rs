// libs/exo_std/src/time/sleep.rs
use super::{Instant, Duration};

/// Fait dormir le thread courant pendant la durée spécifiée
pub fn sleep(duration: Duration) {
    let start = Instant::now();
    let end = start + duration;
    
    while Instant::now() < end {
        // Yield le CPU pour laisser d'autres threads s'exécuter
        yield_cpu();
    }
}

/// Donne le CPU à un autre thread (yield)
fn yield_cpu() {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, utiliser std::thread::yield_now si disponible
        #[cfg(feature = "std")]
        std::thread::yield_now();
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_yield();
            }
            sys_yield();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{seconds, milliseconds};
    
    #[test]
    fn test_sleep_basic() {
        let start = Instant::now();
        sleep(milliseconds(100));
        let elapsed = start.elapsed();
        
        // Vérifier que le sommeil a duré au moins 100ms
        assert!(elapsed >= milliseconds(100));
        
        // Vérifier qu'il n'a pas duré trop longtemps (max 10% de plus)
        assert!(elapsed <= milliseconds(110));
    }
    
    #[test]
    fn test_sleep_zero() {
        let start = Instant::now();
        sleep(Duration::from_nanos(0));
        let elapsed = start.elapsed();
        
        // Le sommeil de 0ns devrait être très rapide
        assert!(elapsed.as_nanos() < 1_000_000); // < 1ms
    }
    
    #[test]
    fn test_sleep_large() {
        let start = Instant::now();
        sleep(seconds(1));
        let elapsed = start.elapsed();
        
        // Vérifier que le sommeil a duré au moins 1s
        assert!(elapsed >= seconds(1));
        
        // Vérifier qu'il n'a pas duré trop longtemps (max 5% de plus)
        assert!(elapsed <= seconds(1) + milliseconds(50));
    }
}





