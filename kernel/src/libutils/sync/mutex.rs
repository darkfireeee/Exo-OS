//! Implémentation d'un mutex adapté au noyau
//! 
//! Ce mutex est conçu pour fonctionner dans un environnement no_std et utilise
//! des instructions atomiques pour la synchronisation.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering, spin_loop_hint};

/// Mutex simple avec attente active (spinlock)
pub struct Mutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for Mutex<T> {}
unsafe impl<T: Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    /// Crée un nouveau mutex
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Tente de verrouiller le mutex
    pub fn try_lock(&self) -> Option<MutexGuard<T>> {
        if self.locked.compare_exchange_weak(
            false, 
            true, 
            Ordering::Acquire, 
            Ordering::Relaxed
        ).is_ok() {
            Some(MutexGuard { mutex: self })
        } else {
            None
        }
    }

    /// Verrouille le mutex, en bloquant jusqu'à ce qu'il soit disponible
    pub fn lock(&self) -> MutexGuard<T> {
        while self.locked.compare_exchange_weak(
            false, 
            true, 
            Ordering::Acquire, 
            Ordering::Relaxed
        ).is_err() {
            // Attendre activement
            spin_loop_hint();
        }
        
        MutexGuard { mutex: self }
    }

    /// Force le déverrouillage du mutex (à utiliser avec précaution)
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

/// Guard qui garantit le déverrouillage automatique du mutex
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.data.get() }
    }
}