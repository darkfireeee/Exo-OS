<<<<<<< Updated upstream
// libs/exo_std/src/sync/condvar.rs
//! Variable de condition pour synchronisation complexe
//!
//! Permet aux threads d'attendre efficacement qu'une condition soit satisfaite.

use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration;
use super::mutex::{Mutex, MutexGuard};
use crate::Result;
use crate::error::SyncError;

/// Variable de condition pour coordination entre threads
///
/// # Exemple
/// ```no_run
/// use exo_std::sync::{Mutex, Condvar};
///
/// let pair = (Mutex::new(false), Condvar::new());
/// let &(ref lock, ref cvar) = &pair;
///
/// // Thread 1
/// {
///     let mut started = lock.lock().unwrap();
///     *started = true;
///     cvar.notify_one();
/// }
///
/// // Thread 2
/// {
///     let mut started = lock.lock().unwrap();
///     while !*started {
///         started = cvar.wait(started).unwrap();
///     }
/// }
/// ```
pub struct Condvar {
    /// Compteur pour wake-ups
    seq: AtomicU32,
}

impl Condvar {
    /// Crée une nouvelle Condvar
    #[inline]
    pub const fn new() -> Self {
        Self {
            seq: AtomicU32::new(0),
        }
    }
    
    /// Attend que la condition soit signalée
    ///
    /// Libère temporairement le mutex et attend. Le mutex est réacquis
    /// avant de retourner.
    ///
    /// # Panics
    /// Panique si le mutex est empoisonné au réveil
    #[inline]
    pub fn wait<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
    ) -> Result<MutexGuard<'a, T>> {
        let mutex = guard.mutex;
        let seq = self.seq.load(Ordering::Acquire);
        
        // Libère le mutex
        drop(guard);
        
        // Attente active optimisée
        let mut backoff = super::mutex::Backoff::new();
        while self.seq.load(Ordering::Acquire) == seq {
            backoff.spin();
            if backoff.should_yield() {
                crate::syscall::thread::yield_now();
            }
            backoff.next();
        }
        
        // Réacquiert le mutex
        mutex.lock().map_err(|_| SyncError::Poisoned.into())
    }
    
    /// Attend avec timeout
    ///
    /// Retourne true si réveillé par notify, false si timeout
    #[inline]
    pub fn wait_timeout<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        dur: Duration,
    ) -> Result<(MutexGuard<'a, T>, bool)> {
        let mutex = guard.mutex;
        let seq = self.seq.load(Ordering::Acquire);
        let start = crate::time::Instant::now();
        
        drop(guard);
        
        let mut backoff = super::mutex::Backoff::new();
        loop {
            if self.seq.load(Ordering::Acquire) != seq {
                // Signal reçu
                let guard = mutex.lock().map_err(|_| SyncError::Poisoned)?;
                return Ok((guard, true));
            }
            
            if start.elapsed() >= dur {
                // Timeout
                let guard = mutex.lock().map_err(|_| SyncError::Poisoned)?;
                return Ok((guard, false));
            }
            
            backoff.spin();
            if backoff.should_yield() {
                crate::syscall::thread::yield_now();
            }
            backoff.next();
        }
    }
    
    /// Attend tant qu'une condition est vraie
    ///
    /// Équivalent à:
    /// ```ignore
    /// while !condition() {
    ///     guard = condvar.wait(guard)?;
    /// }
    /// ```
    #[inline]
    pub fn wait_while<'a, T, F>(
        &self,
        mut guard: MutexGuard<'a, T>,
        mut condition: F,
    ) -> Result<MutexGuard<'a, T>>
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut *guard) {
            guard = self.wait(guard)?;
        }
        Ok(guard)
    }
    
    /// Réveille un thread en attente
    #[inline]
    pub fn notify_one(&self) {
        self.seq.fetch_add(1, Ordering::Release);
    }
    
    /// Réveille tous les threads en attente
    #[inline]
    pub fn notify_all(&self) {
        self.seq.fetch_add(1, Ordering::Release);
    }
}

impl Default for Condvar {
    #[inline]
    fn default() -> Self {
        Self::new()
=======
//! Variable conditionnelle

use core::sync::atomic::{AtomicBool, Ordering};
use super::mutex::{Mutex, MutexGuard};

/// Variable conditionnelle (implémentation simplifiée)
pub struct Condvar {
    flag: AtomicBool,
}

impl Condvar {
    /// Crée une nouvelle condvar
    pub const fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
        }
    }

    /// Attend sur la condvar (nécessite mutex)
    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        // Note: Implémentation simplifiée pour no_std
        // Dans un vrai OS, on utiliserait futex ou équivalent
        let mutex = unsafe { &*((&guard as *const MutexGuard<T>) as *const super::mutex::Mutex<T>) };
        drop(guard);

        // Attend le signal
        while !self.flag.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }

        // Réacquiert le mutex
        mutex.lock()
    }

    /// Notifie un thread
    pub fn notify_one(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Notifie tous les threads
    pub fn notify_all(&self) {
        self.flag.store(true, Ordering::Release);
>>>>>>> Stashed changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
<<<<<<< Updated upstream
    
    #[test]
    fn test_condvar_basic() {
        let mutex = Mutex::new(false);
        let condvar = Condvar::new();
        
        let guard = mutex.lock().unwrap();
        condvar.notify_one();
        let _guard = condvar.wait_timeout(guard, Duration::from_millis(10)).unwrap();
=======

    #[test]
    fn test_condvar_basic() {
        let cv = Condvar::new();
        cv.notify_one();
        cv.notify_all();
>>>>>>> Stashed changes
    }
}
