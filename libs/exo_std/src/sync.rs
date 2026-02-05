// libs/exo_std/src/sync.rs
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Mutex simple (spinlock pour l'instant)
pub struct Mutex<T: ?Sized> {
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

/// Guard pour Mutex - libère automatiquement le verrou lors du drop
pub struct MutexGuard<'a, T: ?Sized + 'a> {
    mutex: &'a Mutex<T>,
}

unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    /// Crée un nouveau Mutex
    pub const fn new(data: T) -> Mutex<T> {
        Mutex {
            lock: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Consomme le Mutex et retourne la donnée interne
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Acquiert le verrou, bloquant jusqu'à ce qu'il soit disponible
    pub fn lock(&self) -> MutexGuard<'_, T> {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Spin - pourrait être amélioré avec pause/yield
            core::hint::spin_loop();
        }
        MutexGuard { mutex: self }
    }

    /// Tente d'acquérir le verrou sans bloquer
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(MutexGuard { mutex: self })
        } else {
            None
        }
    }

    /// Accès non sécurisé à la donnée sous-jacente
    /// 
    /// # Safety
    /// 
    /// Cette fonction est unsafe car elle permet d'obtenir une référence mutable
    /// sans vérifier que le verrou est acquis. L'appelant DOIT garantir:
    /// 
    /// 1. Qu'aucun autre thread n'accède aux données simultanément
    /// 2. Qu'aucun MutexGuard n'existe pour ce Mutex
    /// 3. Qu'aucun autre accès par get_mut_unchecked n'est actif
    /// 
    /// En général, utilisez `lock()` à la place. Cette fonction est réservée
    /// pour des cas très spécifiques comme l'initialisation ou le debugging.
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        // SAFETY: L'appelant garantit l'exclusivité d'accès
        unsafe { &mut *self.data.get() }
    }
    
    /// Accès sécurisé à la donnée mutable (nécessite &mut self)
    pub fn get_mut(&mut self) -> &mut T {
        // SAFETY: &mut self garantit l'exclusivité d'accès
        self.data.get_mut()
    }
}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.mutex.lock.store(false, Ordering::Release);
    }
}
