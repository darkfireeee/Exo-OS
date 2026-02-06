//! Sémaphore compteur avec optimisations

use core::sync::atomic::{AtomicUsize, Ordering};

/// Sémaphore compteur optimisé
///
/// Implémente un sémaphore avec backoff exponentiel pour réduire
/// la contention sur le compteur atomique.
///
/// # Exemple
/// ```no_run
/// use exo_std::sync::Semaphore;
///
/// let sem = Semaphore::new(3);
///
/// sem.acquire();
/// // Section critique
/// sem.release();
/// ```
pub struct Semaphore {
    count: AtomicUsize,
}

impl Semaphore {
    /// Crée un nouveau sémaphore avec une valeur initiale
    ///
    /// # Exemple
    /// ```
    /// use exo_std::sync::Semaphore;
    /// let sem = Semaphore::new(5);
    /// assert_eq!(sem.count(), 5);
    /// ```
    #[inline]
    pub const fn new(initial: usize) -> Self {
        Self {
            count: AtomicUsize::new(initial),
        }
    }

    /// Acquiert une unité du sémaphore (P / wait / down)
    ///
    /// Bloque jusqu'à ce qu'une unité soit disponible.
    /// Utilise un backoff exponentiel pour réduire la contention.
    pub fn acquire(&self) {
        let mut backoff = 1;
        loop {
            let count = self.count.load(Ordering::Acquire);
            if count > 0 {
                match self.count.compare_exchange_weak(
                    count,
                    count - 1,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return,
                    Err(_) => {
                        backoff = 1;
                        continue;
                    }
                }
            }

            for _ in 0..backoff {
                core::hint::spin_loop();
            }

            if backoff < 64 {
                backoff *= 2;
            } else {
                #[cfg(not(feature = "test_mode"))]
                crate::syscall::thread::yield_now();
            }
        }
    }

    /// Acquiert plusieurs unités du sémaphore de manière atomique
    ///
    /// # Panics
    /// Panique si `count` est 0
    pub fn acquire_many(&self, n: usize) {
        assert!(n > 0, "Cannot acquire 0 units");

        let mut backoff = 1;
        loop {
            let count = self.count.load(Ordering::Acquire);
            if count >= n {
                match self.count.compare_exchange_weak(
                    count,
                    count - n,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return,
                    Err(_) => {
                        backoff = 1;
                        continue;
                    }
                }
            }

            for _ in 0..backoff {
                core::hint::spin_loop();
            }

            if backoff < 64 {
                backoff *= 2;
            } else {
                #[cfg(not(feature = "test_mode"))]
                crate::syscall::thread::yield_now();
            }
        }
    }

    /// Tente d'acquérir une unité sans bloquer
    ///
    /// Retourne `true` si acquis, `false` sinon.
    ///
    /// # Exemple
    /// ```
    /// use exo_std::sync::Semaphore;
    /// let sem = Semaphore::new(1);
    ///
    /// assert!(sem.try_acquire());
    /// assert!(!sem.try_acquire());
    /// ```
    #[inline]
    pub fn try_acquire(&self) -> bool {
        let mut count = self.count.load(Ordering::Acquire);
        loop {
            if count == 0 {
                return false;
            }

            match self.count.compare_exchange_weak(
                count,
                count - 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(c) => count = c,
            }
        }
    }

    /// Tente d'acquérir plusieurs unités sans bloquer
    ///
    /// # Panics
    /// Panique si `n` est 0
    #[inline]
    pub fn try_acquire_many(&self, n: usize) -> bool {
        assert!(n > 0, "Cannot acquire 0 units");

        let mut count = self.count.load(Ordering::Acquire);
        loop {
            if count < n {
                return false;
            }

            match self.count.compare_exchange_weak(
                count,
                count - n,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(c) => count = c,
            }
        }
    }

    /// Libère une unité du sémaphore (V / signal / up)
    ///
    /// # Exemple
    /// ```
    /// use exo_std::sync::Semaphore;
    /// let sem = Semaphore::new(0);
    ///
    /// sem.release();
    /// assert_eq!(sem.count(), 1);
    /// ```
    #[inline]
    pub fn release(&self) {
        self.count.fetch_add(1, Ordering::Release);
    }

    /// Libère plusieurs unités du sémaphore
    ///
    /// # Panics
    /// Panique si `n` est 0
    #[inline]
    pub fn release_many(&self, n: usize) {
        assert!(n > 0, "Cannot release 0 units");
        self.count.fetch_add(n, Ordering::Release);
    }

    /// Retourne la valeur actuelle du sémaphore
    ///
    /// Note: Cette valeur peut changer immédiatement après lecture
    ///
    /// # Exemple
    /// ```
    /// use exo_std::sync::Semaphore;
    /// let sem = Semaphore::new(10);
    /// assert_eq!(sem.count(), 10);
    /// ```
    #[inline]
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }
}

unsafe impl Sync for Semaphore {}
unsafe impl Send for Semaphore {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semaphore_basic() {
        let sem = Semaphore::new(2);

        assert_eq!(sem.count(), 2);
        assert!(sem.try_acquire());
        assert_eq!(sem.count(), 1);
        assert!(sem.try_acquire());
        assert_eq!(sem.count(), 0);
        assert!(!sem.try_acquire());

        sem.release();
        assert_eq!(sem.count(), 1);
        assert!(sem.try_acquire());
    }

    #[test]
    fn test_semaphore_many() {
        let sem = Semaphore::new(10);

        assert!(sem.try_acquire_many(5));
        assert_eq!(sem.count(), 5);

        assert!(sem.try_acquire_many(5));
        assert_eq!(sem.count(), 0);

        assert!(!sem.try_acquire_many(1));

        sem.release_many(3);
        assert_eq!(sem.count(), 3);
    }

    #[test]
    fn test_semaphore_blocking() {
        let sem = Semaphore::new(1);

        sem.acquire();
        assert_eq!(sem.count(), 0);
        sem.release();
        assert_eq!(sem.count(), 1);
    }

    #[test]
    #[should_panic(expected = "Cannot acquire 0 units")]
    fn test_acquire_zero_panics() {
        let sem = Semaphore::new(5);
        sem.acquire_many(0);
    }

    #[test]
    #[should_panic(expected = "Cannot release 0 units")]
    fn test_release_zero_panics() {
        let sem = Semaphore::new(5);
        sem.release_many(0);
    }
}
