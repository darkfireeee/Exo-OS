<<<<<<< Updated upstream
// libs/exo_std/src/sync/rwlock.rs
//! Read-Write Lock optimisé avec writer-preference
//!
//! Permet plusieurs lecteurs simultanés ou un seul écrivain exclusif.
//! Utilise writer-preference pour éviter la famine des écrivains.

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};
use super::{LockResult, PoisonError};

/// RwLock permettant plusieurs lecteurs ou un écrivain exclusif
///
/// # Exemple
/// ```no_run
/// use exo_std::sync::RwLock;
///
/// let lock = RwLock::new(vec![1, 2, 3]);
///
/// // Plusieurs lecteurs simultanés
/// {
///     let r1 = lock.read().unwrap();
///     let r2 = lock.read().unwrap();
///     assert_eq!(*r1, vec![1, 2, 3]);
///     assert_eq!(*r2, vec![1, 2, 3]);
/// } // locks libérés
///
/// // Un seul écrivain
/// {
///     let mut w = lock.write().unwrap();
///     w.push(4);
/// }
/// ```
pub struct RwLock<T: ?Sized> {
    /// État: 
    /// - 0 = unlocked
    /// - 1..=0x7FFFFFFF = N readers
    /// - 0x80000000 = writer locked
    /// - 0x80000001..=0xFFFFFFFF = writer locked + N readers en attente
    state: AtomicU32,
    
    /// Indicateur de poison
    #[cfg(feature = "poisoning")]
    poisoned: core::sync::atomic::AtomicBool,
    
    /// Données protégées
    data: UnsafeCell<T>,
}

/// Guard RAII pour lecture
pub struct RwLockReadGuard<'a, T: ?Sized + 'a> {
    lock: &'a RwLock<T>,
}

/// Guard RAII pour écriture
pub struct RwLockWriteGuard<'a, T: ?Sized + 'a> {
    lock: &'a RwLock<T>,
}

const WRITER_BIT: u32 = 1 << 31;
const READER_MASK: u32 = !(1 << 31);
const MAX_READERS: u32 = READER_MASK;

unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}

unsafe impl<T: ?Sized + Sync> Send for RwLockReadGuard<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for RwLockReadGuard<'_, T> {}

unsafe impl<T: ?Sized + Send> Send for RwLockWriteGuard<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for RwLockWriteGuard<'_, T> {}

impl<T> RwLock<T> {
    /// Crée un nouveau RwLock
    #[inline]
    pub const fn new(data: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            #[cfg(feature = "poisoning")]
            poisoned: core::sync::atomic::AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }
    
    /// Consomme le RwLock et retourne la donnée
    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> RwLock<T> {
    /// Acquiert un lock en lecture
    ///
    /// Plusieurs threads peuvent lire simultanément.
    /// Bloque si un écrivain a le lock.
    #[inline]
    pub fn read(&self) -> LockResult<RwLockReadGuard<'_, T>> {
        // Fast path: incrémente le compteur de lecteurs si pas de writer
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            // Vérifie qu'il n'y a pas de writer et qu'on ne dépasse pas MAX_READERS
            if (state & WRITER_BIT == 0) && (state & READER_MASK) < MAX_READERS {
                match self.state.compare_exchange_weak(
                    state,
                    state + 1,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return self.make_read_guard(),
                    Err(s) => state = s,
                }
            } else {
                // Slow path: attente
                break;
            }
        }
        
        self.read_contended()
    }
    
    #[cold]
    fn read_contended(&self) -> LockResult<RwLockReadGuard<'_, T>> {
        let mut backoff = super::mutex::Backoff::new();
        
        loop {
            let state = self.state.load(Ordering::Relaxed);
            
            if (state & WRITER_BIT == 0) && (state & READER_MASK) < MAX_READERS {
                match self.state.compare_exchange_weak(
                    state,
                    state + 1,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return self.make_read_guard(),
                    Err(_) => {}
                }
            }
            
            backoff.spin();
            if backoff.should_yield() {
                crate::syscall::thread::yield_now();
            }
            backoff.next();
        }
    }
    
    /// Acquiert un lock en écriture
    ///
    /// Un seul thread peut écrire à la fois.
    /// Bloque si des lecteurs ou un écrivain ont déjà le lock.
    #[inline]
    pub fn write(&self) -> LockResult<RwLockWriteGuard<'_, T>> {
        // Fast path: acquiert si unlocked
        if self.state
            .compare_exchange(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return self.make_write_guard();
        }
        
        self.write_contended()
    }
    
    #[cold]
    fn write_contended(&self) -> LockResult<RwLockWriteGuard<'_, T>> {
        let mut backoff = super::mutex::Backoff::new();
        
        loop {
            if self.state
                .compare_exchange_weak(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return self.make_write_guard();
            }
            
            backoff.spin();
            if backoff.should_yield() {
                crate::syscall::thread::yield_now();
            }
            backoff.next();
        }
    }
    
    /// Tente d'acquérir un lock en lecture sans bloquer
    #[inline]
    pub fn try_read(&self) -> LockResult<Option<RwLockReadGuard<'_, T>>> {
        let state = self.state.load(Ordering::Relaxed);
        
        if (state & WRITER_BIT == 0) && (state & READER_MASK) < MAX_READERS {
            if self.state
                .compare_exchange(state, state + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                #[cfg(feature = "poisoning")]
                {
                    if self.poisoned.load(Ordering::Relaxed) {
                        return Err(PoisonError::new(Some(RwLockReadGuard { lock: self })));
                    }
                }
                
                return Ok(Some(RwLockReadGuard { lock: self }));
            }
        }
        
        Ok(None)
    }
    
    /// Tente d'acquérir un lock en écriture sans bloquer
    #[inline]
    pub fn try_write(&self) -> LockResult<Option<RwLockWriteGuard<'_, T>>> {
        if self.state
            .compare_exchange(0, WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            #[cfg(feature = "poisoning")]
            {
                if self.poisoned.load(Ordering::Relaxed) {
                    return Err(PoisonError::new(Some(RwLockWriteGuard { lock: self })));
                }
            }
            
            Ok(Some(RwLockWriteGuard { lock: self }))
        } else {
            Ok(None)
        }
    }
    
    #[inline]
    fn make_read_guard(&self) -> LockResult<RwLockReadGuard<'_, T>> {
        #[cfg(feature = "poisoning")]
        {
            if self.poisoned.load(Ordering::Relaxed) {
                return Err(PoisonError::new(RwLockReadGuard { lock: self }));
            }
        }
        
        Ok(RwLockReadGuard { lock: self })
    }
    
    #[inline]
    fn make_write_guard(&self) -> LockResult<RwLockWriteGuard<'_, T>> {
        #[cfg(feature = "poisoning")]
        {
            if self.poisoned.load(Ordering::Relaxed) {
                return Err(PoisonError::new(RwLockWriteGuard { lock: self }));
            }
        }
        
        Ok(RwLockWriteGuard { lock: self })
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
    
    /// Accès mutable direct (requiert &mut self)
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }
}

impl<'a, T: ?Sized> Deref for RwLockReadGuard<'a, T> {
    type Target = T;
    
    #[inline]
=======
//! RwLock (readers-writer lock)

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::ops::{Deref, DerefMut};

const WRITER: usize = 1 << 31;

/// RwLock avec plusieurs lecteurs ou un seul écrivain
pub struct RwLock<T> {
    state: AtomicUsize,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// Crée un nouveau RwLock
    pub const fn new(value: T) -> Self {
        Self {
            state: AtomicUsize::new(0),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquiert en lecture
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        loop {
            let state = self.state.load(Ordering::Acquire);
            if state & WRITER == 0 {
                if self
                    .state
                    .compare_exchange_weak(
                        state,
                        state + 1,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return RwLockReadGuard { lock: self };
                }
            }
            core::hint::spin_loop();
        }
    }

    /// Acquiert en écriture
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        while self
            .state
            .compare_exchange_weak(0, WRITER, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }

        RwLockWriteGuard { lock: self }
    }

    fn unlock_read(&self) {
        self.state.fetch_sub(1, Ordering::Release);
    }

    fn unlock_write(&self) {
        self.state.store(0, Ordering::Release);
    }
}

/// Guard pour lecture
pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

>>>>>>> Stashed changes
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

<<<<<<< Updated upstream
impl<'a, T: ?Sized> Drop for RwLockReadGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // Décrémente le compteur de lecteurs
        self.lock.state.fetch_sub(1, Ordering::Release);
    }
}

impl<'a, T: ?Sized> Deref for RwLockWriteGuard<'a, T> {
    type Target = T;
    
    #[inline]
=======
impl<T> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.unlock_read();
    }
}

/// Guard pour écriture
pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

>>>>>>> Stashed changes
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

<<<<<<< Updated upstream
impl<'a, T: ?Sized> DerefMut for RwLockWriteGuard<'a, T> {
    #[inline]
=======
impl<T> DerefMut for RwLockWriteGuard<'_, T> {
>>>>>>> Stashed changes
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

<<<<<<< Updated upstream
impl<'a, T: ?Sized> Drop for RwLockWriteGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        #[cfg(feature = "poisoning")]
        {
            if core::panic::panicking() {
                self.lock.poisoned.store(true, Ordering::Relaxed);
            }
        }
        
        // Libère le writer bit
        self.lock.state.fetch_and(!WRITER_BIT, Ordering::Release);
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_read() {
            Ok(Some(guard)) => f.debug_struct("RwLock")
                .field("data", &&*guard)
                .finish(),
            _ => f.debug_struct("RwLock")
                .field("data", &"<locked>")
                .finish(),
        }
    }
}

impl<T: Default> Default for RwLock<T> {
    #[inline]
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T> From<T> for RwLock<T> {
    #[inline]
    fn from(data: T) -> Self {
        Self::new(data)
=======
impl<T> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.unlock_write();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rwlock() {
        let lock = RwLock::new(0);
        
        {
            let r1 = lock.read();
            let r2 = lock.read();
            assert_eq!(*r1, 0);
            assert_eq!(*r2, 0);
        }

        {
            let mut w = lock.write();
            *w = 42;
        }

        assert_eq!(*lock.read(), 42);
>>>>>>> Stashed changes
    }
}
