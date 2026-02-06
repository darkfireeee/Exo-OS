// libs/exo_std/src/sync/mutex.rs
//! Mutex optimisé avec backoff exponentiel
//!
//! Cette implémentation fournit:
//! - Backoff exponentiel pour réduire la contention
//! - Fast-path optimisé O(1) pour cas non-contendus
//! - Support optionnel du poisoning
//! - Gestion des panics avec recovery

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use super::{LockResult, PoisonError};

/// Mutex thread-safe avec backoff exponentiel
///
/// # Exemple
/// ```no_run
/// use exo_std::sync::Mutex;
///
/// let mutex = Mutex::new(0);
/// {
///    let mut guard = mutex.lock().unwrap();
///     *guard += 1;
/// }
/// ```
pub struct Mutex<T: ?Sized> {
    locked: AtomicBool,
    #[cfg(feature = "poisoning")]
    poisoned: AtomicBool,
    waiters: AtomicU32,
    data: UnsafeCell<T>,
}

/// Guard RAII pour Mutex
pub struct MutexGuard<'a, T: ?Sized + 'a> {
    pub(crate) mutex: &'a Mutex<T>,
}

unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}

unsafe impl<T: ?Sized + Send> Send for MutexGuard<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

impl<T> Mutex<T> {
    /// Crée un nouveau Mutex
    #[inline]
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            #[cfg(feature = "poisoning")]
            poisoned: AtomicBool::new(false),
            waiters: AtomicU32::new(0),
            data: UnsafeCell::new(data),
        }
    }

    /// Consomme le Mutex et retourne la donnée
    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Acquiert le lock, bloquant jusqu'à disponibilité
    #[inline]
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        if self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return self.make_guard();
        }

        self.lock_contended()
    }

    #[cold]
    fn lock_contended(&self) -> LockResult<MutexGuard<'_, T>> {
        self.waiters.fetch_add(1, Ordering::Relaxed);

        let mut backoff = Backoff::new();

        loop {
            for _ in 0..backoff.iterations() {
                if self.locked
                    .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    self.waiters.fetch_sub(1, Ordering::Relaxed);
                    return self.make_guard();
                }

                backoff.spin();
            }

            if backoff.should_yield() {
                #[cfg(not(feature = "test_mode"))]
                crate::syscall::thread::yield_now();
            }

            backoff.next();
        }
    }

    #[inline]
    fn make_guard(&self) -> LockResult<MutexGuard<'_, T>> {
        #[cfg(feature = "poisoning")]
        {
            if self.poisoned.load(Ordering::Relaxed) {
                return Err(PoisonError::new(MutexGuard { mutex: self }));
            }
        }

        Ok(MutexGuard { mutex: self })
    }

    /// Tente d'acquérir le lock sans bloquer
    #[inline]
    pub fn try_lock(&self) -> LockResult<Option<MutexGuard<'_, T>>> {
        if self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            #[cfg(feature = "poisoning")]
            {
                if self.poisoned.load(Ordering::Relaxed) {
                    return Err(PoisonError::new(Some(MutexGuard { mutex: self })));
                }
            }

            Ok(Some(MutexGuard { mutex: self }))
        } else {
            Ok(None)
        }
    }

    /// Vérifie si empoisonné
    #[inline]
    pub fn is_poisoned(&self) -> bool {
        #[cfg(feature = "poisoning")]
        {
            self.poisoned.load(Ordering::Relaxed)
        }

        #[cfg(not(feature = "poisoning"))]
        {
            false
        }
    }

    /// Accès sécurisé mutable (nécessite &mut self)
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }
}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for MutexGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        #[cfg(feature = "poisoning")]
        {
            if core::panic::panicking() {
                self.mutex.poisoned.store(true, Ordering::Relaxed);
            }
        }

        self.mutex.locked.store(false, Ordering::Release);
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_lock() {
            Ok(Some(guard)) => f.debug_struct("Mutex")
                .field("data", &&*guard)
                .finish(),
            Ok(None) => f.debug_struct("Mutex")
                .field("data", &"<locked>")
                .finish(),
            Err(_) => f.debug_struct("Mutex")
                .field("data", &"<poisoned>")
                .finish(),
        }
    }
}

impl<'a, T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: ?Sized + fmt::Display> fmt::Display for MutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: Default> Default for Mutex<T> {
    #[inline]
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T> From<T> for Mutex<T> {
    #[inline]
    fn from(data: T) -> Self {
        Self::new(data)
    }
}

/// Backoff exponentiel pour réduire la contention
pub(crate) struct Backoff {
    iteration: u32,
}

impl Backoff {
    const MAX_SPIN: u32 = 10;
    const YIELD_THRESHOLD: u32 = 20;

    #[inline]
    pub const fn new() -> Self {
        Self { iteration: 0 }
    }

    #[inline]
    pub fn iterations(&self) -> u32 {
        1 << self.iteration.min(Self::MAX_SPIN)
    }

    #[inline]
    pub fn spin(&self) {
        for _ in 0..(1 << self.iteration.min(6)) {
            core::hint::spin_loop();
        }
    }

    #[inline]
    pub fn should_yield(&self) -> bool {
        self.iteration >= Self::YIELD_THRESHOLD
    }

    #[inline]
    pub fn next(&mut self) {
        self.iteration = self.iteration.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutex_basic() {
        let mutex = Mutex::new(0);
        {
            let mut guard = mutex.lock().unwrap();
            *guard += 1;
        }
        assert_eq!(*mutex.lock().unwrap(), 1);
    }

    #[test]
    fn test_mutex_try_lock() {
        let mutex = Mutex::new(0);
        let _guard1 = mutex.lock().unwrap();

        assert!(mutex.try_lock().unwrap().is_none());
    }

    #[test]
    fn test_mutex_into_inner() {
        let mutex = Mutex::new(42);
        assert_eq!(mutex.into_inner(), 42);
    }

    #[test]
    fn test_mutex_get_mut() {
        let mut mutex = Mutex::new(0);
        *mutex.get_mut() = 10;
        assert_eq!(*mutex.lock().unwrap(), 10);
    }
}
