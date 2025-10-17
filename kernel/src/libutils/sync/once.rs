//! Primitive pour l'initialisation unique
//! 
//! Ce module fournit une implémentation de Once qui garantit qu'une
//! initialisation n'est exécutée qu'une seule fois.

use core::sync::atomic::{AtomicUsize, Ordering};
use core::cell::UnsafeCell;
use core::mem;

/// État possible pour une valeur Once
const INCOMPLETE: usize = 0;
const RUNNING: usize = 1;
const COMPLETE: usize = 2;

/// Primitive pour l'initialisation unique
pub struct Once<T> {
    state: AtomicUsize,
    data: UnsafeCell<mem::MaybeUninit<T>>,
}

unsafe impl<T: Sync> Sync for Once<T> {}

impl<T> Once<T> {
    /// Crée une nouvelle instance de Once
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(INCOMPLETE),
            data: UnsafeCell::new(mem::MaybeUninit::uninit()),
        }
    }

    /// Exécute la fonction d'initialisation si nécessaire et retourne une référence à la valeur
    pub fn call_once<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        // Vérification rapide sans verrouillage
        if self.state.load(Ordering::Acquire) == COMPLETE {
            return unsafe { &*self.data.get().cast() };
        }

        // Tentative d'acquisition du verrou
        if self.state.compare_exchange(
            INCOMPLETE, 
            RUNNING, 
            Ordering::Acquire, 
            Ordering::Relaxed
        ).is_ok() {
            // Nous avons le verrou, initialisons la valeur
            let value = f();
            unsafe {
                (*self.data.get()).write(value);
            }
            
            // Marquons comme terminé
            self.state.store(COMPLETE, Ordering::Release);
            return unsafe { &*self.data.get().cast() };
        }

        // Un autre thread est en train d'initialiser, attendons
        while self.state.load(Ordering::Acquire) != COMPLETE {
            core::hint::spin_loop();
        }

        unsafe { &*self.data.get().cast() }
    }

    /// Vérifie si la valeur a été initialisée
    pub fn is_initialized(&self) -> bool {
        self.state.load(Ordering::Acquire) == COMPLETE
    }
}

impl<T> Drop for Once<T> {
    fn drop(&mut self) {
        if self.state.load(Ordering::Relaxed) == COMPLETE {
            unsafe {
                (*self.data.get()).assume_init_drop();
            }
        }
    }
}