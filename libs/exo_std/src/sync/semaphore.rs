//! Sémaphore

use core::sync::atomic::{AtomicUsize, Ordering};

/// Sémaphore compteur
pub struct Semaphore {
    count: AtomicUsize,
}

impl Semaphore {
    /// Crée un nouveau sémaphore
    pub const fn new(initial: usize) -> Self {
        Self {
            count: AtomicUsize::new(initial),
        }
    }

    /// Acquiert (P / wait)
    pub fn acquire(&self) {
        loop {
            let count = self.count.load(Ordering::Acquire);
            if count > 0 {
                if self
                    .count
                    .compare_exchange_weak(
                        count,
                        count - 1,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return;
                }
            }
            core::hint::spin_loop();
        }
    }

    /// Tente d'acquérir (non-bloquant)
    pub fn try_acquire(&self) -> bool {
        let count = self.count.load(Ordering::Acquire);
        if count > 0 {
            self.count
                .compare_exchange(
                    count,
                    count - 1,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
        } else {
            false
        }
    }

    /// Libère (V / signal)
    pub fn release(&self) {
        self.count.fetch_add(1, Ordering::Release);
    }

    /// Valeur actuelle
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semaphore() {
        let sem = Semaphore::new(2);
        
        assert!(sem.try_acquire());
        assert!(sem.try_acquire());
        assert!(!sem.try_acquire());
        
        sem.release();
        assert!(sem.try_acquire());
    }
}
