// libs/exo_std/src/sync/once.rs
//! Initialisation unique thread-safe
//!
//! Permet d'exécuter du code exactement une fois, même avec plusieurs threads.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};
use core::mem::MaybeUninit;

/// États de Once
const INCOMPLETE: u8 = 0;
const RUNNING: u8 = 1;
const COMPLETE: u8 = 2;

/// Primitive pour exécution unique
///
/// # Exemple
/// ```no_run
/// use exo_std::sync::Once;
///
/// static INIT: Once = Once::new();
///
/// INIT.call_once(|| {
///     // Code exécuté exactement une fois
/// });
/// ```
pub struct Once {
    state: AtomicU8,
}

impl Once {
    /// Crée un nouveau Once
    #[inline]
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(INCOMPLETE),
        }
    }
    
    /// Exécute la fonction exactement une fois
    ///
    /// Si plusieurs threads appellent simultanément, un seul exécutera
    /// la fonction. Les autres attendront la complétion.
    #[inline]
    pub fn call_once<F>(&self, f: F)
    where
        F: FnOnce(),
    {
        // Fast path: déjà complété
        if self.is_completed() {
            return;
        }
        
        // Slow path: tentative d'acquisition
        self.call_once_slow(f);
    }
    
    #[cold]
    fn call_once_slow<F>(&self, f: F)
    where
        F: FnOnce(),
    {
        // Tente d'acquérir le droit d'exécution
        if self.state
            .compare_exchange(INCOMPLETE, RUNNING, Ordering::Acquire, Ordering::Acquire)
            .is_ok()
        {
            // On a acquis, exécuter la fonction
            f();
            
            // Marquer comme complété
            self.state.store(COMPLETE, Ordering::Release);
        } else {
            // Quelqu'un d'autre exécute, attendre la complétion
            let mut backoff = super::mutex::Backoff::new();
            while !self.is_completed() {
                backoff.spin();
                if backoff.should_yield() {
                    crate::syscall::thread::yield_now();
                }
                backoff.next();
            }
        }
    }
    
    /// Vérifie si l'initialisation est complétée
    #[inline]
    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == COMPLETE
    }
}

impl Default for Once {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Cellule avec initialisation unique thread-safe
///
/// # Exemple
/// ```no_run
/// use exo_std::sync::OnceLock;
///
/// static CONFIG: OnceLock<String> = OnceLock::new();
///
/// fn get_config() -> &'static String {
///     CONFIG.get_or_init(|| {
///         String::from("default config")
///     })
/// }
/// ```
pub struct OnceLock<T> {
    once: Once,
    value: UnsafeCell<MaybeUninit<T>>,
}

// Safety: OnceLock est Sync si T est Sync + Send
unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}
unsafe impl<T: Send> Send for OnceLock<T> {}

impl<T> OnceLock<T> {
    /// Crée un nouveau OnceLock vide
    #[inline]
    pub const fn new() -> Self {
        Self {
            once: Once::new(),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
    
    /// Obtient la valeur ou l'initialise
    ///
    /// Si la valeur n'existe pas, appelle `f` pour l'initialiser.
    /// Retourne une référence à la valeur.
    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        // Fast path: déjà initialisé
        if self.once.is_completed() {
            return unsafe { self.get_unchecked() };
        }
        
        // Slow path: initialisation
        self.once.call_once(|| {
            let value = f();
            unsafe {
                (*self.value.get()).write(value);
            }
        });
        
        unsafe { self.get_unchecked() }
    }
    
    /// Tente d'initialiser avec une valeur
    ///
    /// Retourne Ok(()) si initialisé avec succès, Err(value) si déjà initialisé.
    pub fn set(&self, value: T) -> Result<(), T> {
        if self.once.is_completed() {
            return Err(value);
        }
        
        let mut value = Some(value);
        self.once.call_once(|| {
            unsafe {
                (*self.value.get()).write(value.take().unwrap());
            }
        });
        
        if let Some(value) = value {
            Err(value)
        } else {
            Ok(())
        }
    }
    
    /// Obtient la valeur si initialisée
    #[inline]
    pub fn get(&self) -> Option<&T> {
        if self.once.is_completed() {
            Some(unsafe { self.get_unchecked() })
        } else {
            None
        }
    }
    
    /// Obtient la valeur sans vérification
    ///
    /// # Safety
    /// La valeur doit avoir été initialisée
    #[inline]
    unsafe fn get_unchecked(&self) -> &T {
        (*self.value.get()).assume_init_ref()
    }
    
    /// Consomme le OnceLock et retourne la valeur si initialisée
    #[inline]
    pub fn into_inner(mut self) -> Option<T> {
        if self.once.is_completed() {
            Some(unsafe { (*self.value.get_mut()).assume_init_read() })
        } else {
            None
        }
    }
}

impl<T> Default for OnceLock<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for OnceLock<T> {
    fn drop(&mut self) {
        if self.once.is_completed() {
            unsafe {
                (*self.value.get_mut()).assume_init_drop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_once() {
        let once = Once::new();
        let mut count = 0;
        
        once.call_once(|| count += 1);
        once.call_once(|| count += 1);
        
        assert_eq!(count, 1);
    }
    
    #[test]
    fn test_once_lock() {
        let lock = OnceLock::new();
        
        assert_eq!(lock.get(), None);
        
        let value = lock.get_or_init(|| 42);
        assert_eq!(*value, 42);
        
assert_eq!(lock.get(), Some(&42));
    }
}
