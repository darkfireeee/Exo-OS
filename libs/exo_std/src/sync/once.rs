// libs/exo_std/src/sync/once.rs
//! Initialisation unique thread-safe
//!
//! Permet d'exécuter du code exactement une fois, même avec plusieurs threads.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};

/// États de Once
const INCOMPLETE: u8 = 0;
const RUNNING: u8 = 1;
const COMPLETE: u8 = 2;

/// Primitive de synchronisation pour initialisation unique
///
/// # Exemples
///
/// ```
/// use exo_std::sync::Once;
///
/// static INIT: Once = Once::new();
///
/// INIT.call_once(|| {
///     // Code exécuté exactement une fois
///     println!("Initialized!");
/// });
/// ```
pub struct Once {
    state: AtomicU8,
}

impl Once {
    /// Crée un nouveau `Once` non initialisé
    #[inline]
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(INCOMPLETE),
        }
    }

    /// Exécute la fonction `f` exactement une fois
    ///
    /// Si plusieurs threads appellent `call_once` simultanément,
    /// un seul thread exécutera `f`, les autres attendront la fin.
    ///
    /// # Garanties
    ///
    /// - `f` est appelée exactement une fois
    /// - Les appels suivants ne font rien
    /// - Tous les threads voient les effets de `f` après retour
    pub fn call_once<F>(&self, f: F)
    where
        F: FnOnce(),
    {
        if self.is_completed() {
            return;
        }

        self.call_once_slow(f);
    }

    /// Chemin lent pour l'initialisation
    #[cold]
    fn call_once_slow<F>(&self, f: F)
    where
        F: FnOnce(),
    {
        loop {
            match self.state.compare_exchange(
                INCOMPLETE,
                RUNNING,
                Ordering::Acquire,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    f();
                    self.state.store(COMPLETE, Ordering::Release);
                    return;
                }
                Err(RUNNING) => {
                    let mut backoff = 1;
                    while self.state.load(Ordering::Acquire) == RUNNING {
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
                Err(COMPLETE) => {
                    return;
                }
                Err(invalid) => {
                    panic!("Once in invalid state: {}", invalid);
                }
            }
        }
    }

    /// Retourne `true` si l'initialisation est complète
    #[inline]
    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == COMPLETE
    }
}

impl Default for Once {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Sync for Once {}
unsafe impl Send for Once {}

/// Once avec données associées
///
/// Permet de stocker le résultat de l'initialisation.
///
/// # Exemples
///
/// ```
/// use exo_std::sync::OnceLock;
///
/// static CONFIG: OnceLock<&str> = OnceLock::new();
///
/// CONFIG.get_or_init(|| "config value");
/// assert_eq!(CONFIG.get(), Some(&"config value"));
/// ```
pub struct OnceLock<T> {
    once: Once,
    value: UnsafeCell<Option<T>>,
}

impl<T> OnceLock<T> {
    /// Crée un nouveau `OnceLock` vide
    #[inline]
    pub const fn new() -> Self {
        Self {
            once: Once::new(),
            value: UnsafeCell::new(None),
        }
    }

    /// Récupère la valeur si elle existe
    #[inline]
    pub fn get(&self) -> Option<&T> {
        if self.once.is_completed() {
            unsafe { (*self.value.get()).as_ref() }
        } else {
            None
        }
    }

    /// Récupère ou initialise la valeur
    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        if let Some(value) = self.get() {
            return value;
        }

        self.get_or_init_slow(f)
    }

    /// Chemin lent pour get_or_init
    #[cold]
    fn get_or_init_slow<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        self.once.call_once(|| unsafe {
            *self.value.get() = Some(f());
        });

        unsafe { (*self.value.get()).as_ref().unwrap() }
    }

    /// Tente de définir la valeur
    ///
    /// Retourne `Ok(())` si la valeur a été définie, `Err(value)` si déjà initialisée.
    pub fn set(&self, value: T) -> Result<(), T> {
        if self.once.is_completed() {
            return Err(value);
        }

        let mut value = Some(value);
        self.once.call_once(|| unsafe {
            *self.value.get() = value.take();
        });

        match value {
            None => Ok(()),
            Some(v) => Err(v),
        }
    }

    /// Récupère la valeur mutable (uniquement si `&mut self`)
    #[inline]
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.value.get_mut().as_mut()
    }

    /// Consomme le `OnceLock` et retourne la valeur
    #[inline]
    pub fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }
}

impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<T: Send + Sync> Sync for OnceLock<T> {}
unsafe impl<T: Send> Send for OnceLock<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_once_basic() {
        static ONCE: Once = Once::new();
        static mut COUNTER: u32 = 0;

        assert!(!ONCE.is_completed());

        ONCE.call_once(|| unsafe {
            COUNTER += 1;
        });

        assert!(ONCE.is_completed());
        assert_eq!(unsafe { COUNTER }, 1);

        ONCE.call_once(|| unsafe {
            COUNTER += 1;
        });

        assert_eq!(unsafe { COUNTER }, 1);
    }

    #[test]
    fn test_once_lock() {
        let cell: OnceLock<u32> = OnceLock::new();

        assert_eq!(cell.get(), None);

        let value = cell.get_or_init(|| 42);
        assert_eq!(value, &42);

        let value2 = cell.get_or_init(|| 100);
        assert_eq!(value2, &42);
    }

    #[test]
    fn test_once_lock_set() {
        let cell: OnceLock<&str> = OnceLock::new();

        assert!(cell.set("hello").is_ok());
        assert_eq!(cell.get(), Some(&"hello"));

        assert!(cell.set("world").is_err());
        assert_eq!(cell.get(), Some(&"hello"));
    }

    #[test]
    fn test_once_lock_into_inner() {
        let cell = OnceLock::new();
        cell.set(123).unwrap();

        assert_eq!(cell.into_inner(), Some(123));
    }
}
