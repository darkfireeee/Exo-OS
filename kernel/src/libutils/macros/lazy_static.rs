//! Version `no_std` de lazy_static
//! 
//! Ce module fournit une implémentation de lazy_static adaptée pour no_std,
//! permettant l'initialisation paresseuse de variables statiques.

use core::sync::atomic::{AtomicBool, Ordering};
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ops::Deref;

/// Macro pour créer une variable statique initialisée paresseusement
#[macro_export]
macro_rules! lazy_static {
    ($(#[$meta:meta])* static ref $N:ident : $T:ty = $e:expr; $($t:tt)*) => {
        $crate::lazy_static!(@WRAP $(#[$meta])* static ref $N : $T = $e);
        $crate::lazy_static!($($t)*);
    };
    ($(#[$meta:meta])* static mut ref $N:ident : $T:ty = $e:expr; $($t:tt)*) => {
        $crate::lazy_static!(@WRAP $(#[$meta])* static mut ref $N : $T = $e);
        $crate::lazy_static!($($t)*);
    };
    () => ();
    (@WRAP $(#[$meta:meta])* static ref $N:ident : $T:ty = $e:expr) => {
        $(#[$meta])*
        static $N: $crate::macros::lazy_static::LazyStatic<$T> = {
            $crate::macros::lazy_static::LazyStatic {
                _cell: $crate::macros::lazy_static::UnsafeCell::new(
                    $crate::macros::lazy_static::MaybeUninit::uninit()
                ),
                _init: $crate::macros::lazy_static::AtomicBool::new(false),
                _init_fn: || { $e },
            }
        };
        impl $crate::macros::lazy_static::Deref for $N {
            type Target = $T;
            fn deref(&self) -> &$T {
                unsafe {
                    if !self._init.load($crate::macros::lazy_static::Ordering::Acquire) {
                        // Initialisation paresseuse
                        let value = (self._init_fn)();
                        self._cell.get().write(value);
                        self._init.store(true, $crate::macros::lazy_static::Ordering::Release);
                    }
                    &*self._cell.get()
                }
            }
        }
    };
    (@WRAP $(#[$meta:meta])* static mut ref $N:ident : $T:ty = $e:expr) => {
        $(#[$meta])*
        static mut $N: $crate::macros::lazy_static::LazyStatic<$T> = {
            $crate::macros::lazy_static::LazyStatic {
                _cell: $crate::macros::lazy_static::UnsafeCell::new(
                    $crate::macros::lazy_static::MaybeUninit::uninit()
                ),
                _init: $crate::macros::lazy_static::AtomicBool::new(false),
                _init_fn: || { $e },
            }
        };
        impl $N {
            /// Obtient une référence mutable à la valeur, en l'initialisant si nécessaire
            ///
            /// # Safety
            /// Cette fonction est unsafe car elle retourne une référence mutable à une
            /// variable statique, ce qui peut causer des problèmes de concurrence.
            pub unsafe fn get_mut(&self) -> &mut $T {
                if !self._init.load($crate::macros::lazy_static::Ordering::Acquire) {
                    // Initialisation paresseuse
                    let value = (self._init_fn)();
                    self._cell.get().write(value);
                    self._init.store(true, $crate::macros::lazy_static::Ordering::Release);
                }
                &mut *self._cell.get()
            }
        }
        impl $crate::macros::lazy_static::Deref for $N {
            type Target = $T;
            fn deref(&self) -> &$T {
                unsafe {
                    if !self._init.load($crate::macros::lazy_static::Ordering::Acquire) {
                        // Initialisation paresseuse
                        let value = (self._init_fn)();
                        self._cell.get().write(value);
                        self._init.store(true, $crate::macros::lazy_static::Ordering::Release);
                    }
                    &*self._cell.get()
                }
            }
        }
    };
}

/// Structure interne pour lazy_static
pub struct LazyStatic<T> {
    _cell: UnsafeCell<MaybeUninit<T>>,
    _init: AtomicBool,
    _init_fn: fn() -> T,
}

unsafe impl<T: Sync> Sync for LazyStatic<T> {}