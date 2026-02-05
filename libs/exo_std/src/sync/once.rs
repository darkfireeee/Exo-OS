<<<<<<< Updated upstream
// libs/exo_std/src/sync/once.rs
//! Initialisation unique thread-safe
//!
//! Permet d'exécuter du code exactement une fois, même avec plusieurs threads.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};
use core::mem::MaybeUninit;

/// États de Once
=======
//! Initialisation à usage unique (Once)
//!
//! Garantit qu'une fonction d'initialisation est appelée exactement une fois,
//! même en présence de concurrence multi-thread.

use core::sync::atomic::{AtomicU8, Ordering};
use core::cell::UnsafeCell;

/// États du Once
>>>>>>> Stashed changes
const INCOMPLETE: u8 = 0;
const RUNNING: u8 = 1;
const COMPLETE: u8 = 2;

<<<<<<< Updated upstream
/// Primitive pour exécution unique
///
/// # Exemple
/// ```no_run
=======
/// Primitive de synchronisation pour initialisation unique
///
/// # Exemples
///
/// ```
>>>>>>> Stashed changes
/// use exo_std::sync::Once;
///
/// static INIT: Once = Once::new();
///
/// INIT.call_once(|| {
///     // Code exécuté exactement une fois
<<<<<<< Updated upstream
=======
///     println!("Initialized!");
>>>>>>> Stashed changes
/// });
/// ```
pub struct Once {
    state: AtomicU8,
}

impl Once {
<<<<<<< Updated upstream
    /// Crée un nouveau Once
=======
    /// Crée un nouveau `Once` non initialisé
>>>>>>> Stashed changes
    #[inline]
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(INCOMPLETE),
        }
    }
<<<<<<< Updated upstream
    
    /// Exécute la fonction exactement une fois
    ///
    /// Si plusieurs threads appellent simultanément, un seul exécutera
    /// la fonction. Les autres attendront la complétion.
    #[inline]
=======

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
>>>>>>> Stashed changes
    pub fn call_once<F>(&self, f: F)
    where
        F: FnOnce(),
    {
<<<<<<< Updated upstream
        // Fast path: déjà complété
        if self.is_completed() {
            return;
        }
        
        // Slow path: tentative d'acquisition
        self.call_once_slow(f);
    }
    
=======
        // Fast path: déjà initialisé
        if self.is_completed() {
            return;
        }

        self.call_once_slow(f);
    }

    /// Chemin lent pour l'initialisation
>>>>>>> Stashed changes
    #[cold]
    fn call_once_slow<F>(&self, f: F)
    where
        F: FnOnce(),
    {
<<<<<<< Updated upstream
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
=======
        loop {
            match self.state.compare_exchange(
                INCOMPLETE,
                RUNNING,
                Ordering::Acquire,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // Ce thread fait l'initialisation
                    f();
                    self.state.store(COMPLETE, Ordering::Release);
                    return;
                }
                Err(RUNNING) => {
                    // Un autre thread initialise, attendre
                    while self.state.load(Ordering::Acquire) == RUNNING {
                        core::hint::spin_loop();
                    }
                }
                Err(COMPLETE) => {
                    // Déjà complété par un autre thread
                    return;
                }
                Err(_) => unreachable!(),
            }
        }
    }

    /// Retourne `true` si l'initialisation est complète
>>>>>>> Stashed changes
    #[inline]
    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == COMPLETE
    }
}

impl Default for Once {
<<<<<<< Updated upstream
    #[inline]
=======
>>>>>>> Stashed changes
    fn default() -> Self {
        Self::new()
    }
}

<<<<<<< Updated upstream
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
=======
// Once est thread-safe
unsafe impl Sync for Once {}
unsafe impl Send for Once {}

/// Once avec données associées
///
/// Permet de stocker le résultat de l'initialisation.
///
/// # Exemples
///
/// ```
/// use exo_std::sync::OnceCell;
///
/// static CONFIG: OnceCell<&str> = OnceCell::new();
///
/// CONFIG.get_or_init(|| "config value");
/// assert_eq!(CONFIG.get(), Some(&"config value"));
/// ```
pub struct OnceCell<T> {
    once: Once,
    value: UnsafeCell<Option<T>>,
}

impl<T> OnceCell<T> {
    /// Crée un nouveau `OnceCell` vide
>>>>>>> Stashed changes
    #[inline]
    pub const fn new() -> Self {
        Self {
            once: Once::new(),
<<<<<<< Updated upstream
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
    
    /// Obtient la valeur ou l'initialise
    ///
    /// Si la valeur n'existe pas, appelle `f` pour l'initialiser.
    /// Retourne une référence à la valeur.
=======
            value: UnsafeCell::new(None),
        }
    }

    /// Récupère la valeur si elle existe
    #[inline]
    pub fn get(&self) -> Option<&T> {
        if self.once.is_completed() {
            // SAFETY: La valeur est initialisée et ne change plus
            unsafe { (*self.value.get()).as_ref() }
        } else {
            None
        }
    }

    /// Récupère ou initialise la valeur
>>>>>>> Stashed changes
    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
<<<<<<< Updated upstream
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
=======
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
        self.once.call_once(|| {
            // SAFETY: Appelé exactement une fois, accès exclusif
            unsafe {
                *self.value.get() = Some(f());
            }
        });

        // SAFETY: La valeur est maintenant initialisée
        unsafe { (*self.value.get()).as_ref().unwrap() }
    }

    /// Tente de définir la valeur
    ///
    /// Retourne `Ok(())` si la valeur a été définie, `Err(value)` si déjà initialisée.
>>>>>>> Stashed changes
    pub fn set(&self, value: T) -> Result<(), T> {
        if self.once.is_completed() {
            return Err(value);
        }
<<<<<<< Updated upstream
        
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
=======

        let mut value = Some(value);
        self.once.call_once(|| {
            // SAFETY: Appelé exactement une fois
            unsafe {
                *self.value.get() = value.take();
            }
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

    /// Consomme le `OnceCell` et retourne la valeur
    #[inline]
    pub fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }
}

impl<T> Default for OnceCell<T> {
>>>>>>> Stashed changes
    fn default() -> Self {
        Self::new()
    }
}

<<<<<<< Updated upstream
impl<T> Drop for OnceLock<T> {
    fn drop(&mut self) {
        if self.once.is_completed() {
            unsafe {
                (*self.value.get_mut()).assume_init_drop();
            }
        }
    }
}
=======
// OnceCell<T> est thread-safe si T est Send + Sync
unsafe impl<T: Send + Sync> Sync for OnceCell<T> {}
unsafe impl<T: Send> Send for OnceCell<T> {}
>>>>>>> Stashed changes

#[cfg(test)]
mod tests {
    use super::*;
<<<<<<< Updated upstream
    
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
=======

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

        // Deuxième appel ne fait rien
        ONCE.call_once(|| unsafe {
            COUNTER += 1;
        });

        assert_eq!(unsafe { COUNTER }, 1);
    }

    #[test]
    fn test_once_cell() {
        let cell: OnceCell<u32> = OnceCell::new();

        assert_eq!(cell.get(), None);

        let value = cell.get_or_init(|| 42);
        assert_eq!(value, &42);

        let value2 = cell.get_or_init(|| 100);
        assert_eq!(value2, &42); // Toujours la première valeur
    }

    #[test]
    fn test_once_cell_set() {
        let cell: OnceCell<&str> = OnceCell::new();

        assert!(cell.set("hello").is_ok());
        assert_eq!(cell.get(), Some(&"hello"));

        // Tentative de redéfinition échoue
        assert!(cell.set("world").is_err());
        assert_eq!(cell.get(), Some(&"hello"));
    }

    #[test]
    fn test_once_cell_into_inner() {
        let cell = OnceCell::new();
        cell.set(123).unwrap();

        assert_eq!(cell.into_inner(), Some(123));
>>>>>>> Stashed changes
    }
}
