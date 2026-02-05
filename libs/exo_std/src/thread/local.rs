//! Thread-local storage (TLS)

use core::cell::Cell;
use core::fmt;

/// Erreur d'accès TLS
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessError;

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "thread local storage access error")
    }
}

/// Clé pour thread-local storage
pub struct LocalKey<T: 'static> {
    inner: fn() -> &'static Cell<Option<T>>,
}

impl<T: 'static> LocalKey<T> {
    #[doc(hidden)]
    pub const unsafe fn new(inner: fn() -> &'static Cell<Option<T>>) -> Self {
        Self { inner }
    }

    /// Accède à la valeur TLS
    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let cell = (self.inner)();
        unsafe {
            match cell.as_ptr().as_ref() {
                Some(Some(ref value)) => f(value),
                _ => panic!("thread local storage not initialized"),
            }
        }
    }

    /// Essaie d'accéder à la valeur TLS
    pub fn try_with<F, R>(&'static self, f: F) -> Result<R, AccessError>
    where
        F: FnOnce(&T) -> R,
    {
        let cell = (self.inner)();
        unsafe {
            match cell.as_ptr().as_ref() {
                Some(Some(ref value)) => Ok(f(value)),
                _ => Err(AccessError),
            }
        }
    }
}

impl<T: 'static> fmt::Debug for LocalKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalKey").finish_non_exhaustive()
    }
}

/// Macro pour déclarer une variable thread-local
#[macro_export]
macro_rules! thread_local {
    ($($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr;)*) => {
        $(
            $(#[$attr])*
            $vis static $name: $crate::thread::LocalKey<$t> = {
                fn __init() -> &'static core::cell::Cell<Option<$t>> {
                    // Note: Implémentation simplifiée utilisant une seule Cell statique
                    // Une vraie implémentation TLS nécessite:
                    // - Support kernel pour stocker des données par thread
                    // - API pour allouer/libérer des slots TLS
                    // - Mécanisme de nettoyage lors de la destruction du thread
                    // Cette version fonctionne mais partage la valeur entre tous les threads
                    thread_local!(@impl $init)
                }
                unsafe { $crate::thread::LocalKey::new(__init) }
            };
        )*
    };
    
    (@impl $init:expr) => {{
        use core::cell::Cell;
        static STORAGE: Cell<Option<_>> = Cell::new(Some($init));
        &STORAGE
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static TEST_VAR: Cell<usize> = Cell::new(0);
    }

    #[test]
    fn test_thread_local() {
        TEST_VAR.with(|v| {
            assert_eq!(v.get(), 0);
            v.set(42);
            assert_eq!(v.get(), 42);
        });
    }

    #[test]
    fn test_try_with() {
        let result = TEST_VAR.try_with(|v| v.get());
        assert!(result.is_ok());
    }
}
