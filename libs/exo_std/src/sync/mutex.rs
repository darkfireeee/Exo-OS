// libs/exo_std/src/sync/mutex.rs
//! Mutex optimisé avec backoff exponentiel et poisoning
//!
//! Cette implémentation fournit :
//! - Backoff exponentiel pour réduire la contention
//! - Fast-path pour cas non-contendus
//! - Support optionnel du poisoning
//! - Optimisations de cache-line

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
///     let mut guard = mutex.lock().unwrap();
///     *guard += 1;
/// } // lock automatiquement libéré
/// ```
pub struct Mutex<T: ?Sized> {
    /// État du lock (false = libre, true = acquis)
    locked: AtomicBool,
    /// Indicateur de poison (si un thread panic avec le lock)
    #[cfg(feature = "poisoning")]
    poisoned: AtomicBool,
    /// Compteur de threads en attente (pour optimisations futures)
    waiters: AtomicU32,
    /// Données protégées
    data: UnsafeCell<T>,
}

/// Guard RAII pour Mutex - libère automatiquement le lock au drop
pub struct MutexGuard<'a, T: ?Sized + 'a> {
    mutex: &'a Mutex<T>,
}

// Safety: Mutex peut être partagé entre threads si T est Send
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}

// Safety: MutexGuard peut être envoyé entre threads si T est Send
unsafe impl<T: ?Sized + Send> Send for MutexGuard<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

impl<T> Mutex<T> {
    /// Crée un nouveau Mutex
    ///
    /// # Exemple
    /// ```no_run
    /// use exo_std::sync::Mutex;
    /// let mutex = Mutex::new(42);
    /// ```
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
    
    /// Consomme le Mutex et retourne la donnée interne
    ///
    /// # Exemple
    /// ```no_run
    /// use exo_std::sync::Mutex;
    /// let mutex = Mutex::new(42);
    /// let value = mutex.into_inner();
    /// assert_eq!(value, 42);
    /// ```
    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Acquiert le lock, bloquant jusqu'à sa disponibilité
    ///
    /// Retourne un guard RAII qui libérera automatiquement le lock.
    /// Si le mutex est empoisonné (un thread a paniqué avec le lock),
    /// retourne une erreur mais donne quand même accès aux données.
    ///
    /// # Panics
    /// Peut paniquer si le lock est déjà acquis par le thread actuel (deadlock)
    ///
    /// # Exemple
    /// ```no_run
    /// use exo_std::sync::Mutex;
    /// let mutex = Mutex::new(0);
    /// let mut guard = mutex.lock().unwrap();
    /// *guard += 1;
    /// ```
    #[inline]
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        // Fast path: tentative d'acquisition sans contention
        if self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return self.make_guard();
        }
        
        // Slow path: backoff exponentiel
        self.lock_contended()
    }
    
    /// Path lent avec backoff exponentiel
    #[cold]
    fn lock_contended(&self) -> LockResult<MutexGuard<'_, T>> {
        self.waiters.fetch_add(1, Ordering::Relaxed);
        
        let mut backoff = Backoff::new();
        
        loop {
            // Tentative d'acquisition avec backoff
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
            
            // Si toujours contention, yield au scheduler
            if backoff.should_yield() {
                crate::syscall::thread::yield_now();
            }
            
            backoff.next();
        }
    }
    
    /// Crée un MutexGuard (vérifie poisoning si activé)
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
    ///
    /// Retourne None si le lock n'est pas immédiatement disponible.
    ///
    /// # Exemple
    /// ```no_run
    /// use exo_std::sync::Mutex;
    /// let mutex = Mutex::new(0);
    /// if let Some(guard) = mutex.try_lock().ok().flatten() {
    ///     // Lock acquis
    /// } else {
    ///     // Lock déjà pris
    /// }
    /// ```
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
    
    /// Vérifie si le mutex est empoisonné
    #[cfg(feature = "poisoning")]
    #[inline]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Relaxed)
    }
    
    #[cfg(not(feature = "poisoning"))]
    #[inline]
    pub fn is_poisoned(&self) -> bool {
        false
    }
    
    /// Accès sécurisé à la donnée mutable (nécessite &mut self)
    ///
    /// Comme on a une référence exclusive, pas besoin de lock.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }
}

impl<'a, T: ?Sized> MutexGuard<'a, T> {
    /// Marque le mutex comme empoisonné
    #[cfg(feature = "poisoning")]
    fn poison(&self) {
        self.mutex.poisoned.store(true, Ordering::Relaxed);
    }
}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;
    
    #[inline]
    fn deref(&self) -> &T {
        // Safety: On a le lock, accès exclusif garanti
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // Safety: On a le lock, accès exclusif garanti
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for MutexGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // Vérifie si on est en train de paniquer
        #[cfg(feature = "poisoning")]
        {
            if core::panic::panicking() {
                self.poison();
            }
        }
        
        // Libère le lock
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
struct Backoff {
    iteration: u32,
}

impl Backoff {
    const MAX_SPIN: u32 = 10;
    const YIELD_THRESHOLD: u32 = 20;
    
    #[inline]
    const fn new() -> Self {
        Self { iteration: 0 }
    }
    
    #[inline]
    fn iterations(&self) -> u32 {
        1 << self.iteration.min(Self::MAX_SPIN)
    }
    
    #[inline]
    fn spin(&self) {
        for _ in 0..(1 << self.iteration.min(6)) {
            core::hint::spin_loop();
        }
    }
    
    #[inline]
    fn should_yield(&self) -> bool {
        self.iteration >= Self::YIELD_THRESHOLD
    }
    
    #[inline]
    fn next(&mut self) {
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
        
        // Doit échouer car déjà locked
        assert!(mutex.try_lock().unwrap().is_none());
    }
    
    #[test]
    fn test_mutex_into_inner() {
        let mutex = Mutex::new(42);
        assert_eq!(mutex.into_inner(), 42);
    }
}
