//! fixed_vec.rs — Vecteur à capacité fixe sans allocation heap (#![no_std])
//!
//! Conforme à la règle OOM-02 d'Exo-OS :
//! Toute tentative d'insertion au-delà de la capacité échoue proprement
//! (fail-closed) via `try_push()` en retournant la valeur intacte dans `Err(T)`.

use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};

/// Vecteur stocké en pile/inline de taille maximale `N`
pub struct FixedVec<T, const N: usize> {
    data: [MaybeUninit<T>; N],
    len: usize,
}

impl<T, const N: usize> FixedVec<T, N> {
    /// Crée un nouveau `FixedVec` vide
    pub const fn new() -> Self {
        Self {
            // SAFETY: Un tableau de `MaybeUninit` non initialisé est valide.
            data: unsafe { MaybeUninit::uninit().assume_init() },
            len: 0,
        }
    }

    /// Tente d'insérer un élément. Retourne `Err(value)` si la capacité `N` est atteinte.
    #[inline]
    pub fn try_push(&mut self, value: T) -> Result<(), T> {
        if self.len >= N {
            Err(value)
        } else {
            self.data[self.len].write(value);
            self.len += 1;
            Ok(())
        }
    }

    /// Retire le dernier élément inséré, ou `None` si vide.
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            None
        } else {
            self.len -= 1;
            // SAFETY: L'élément à `self.len` a été initialisé lors d'un `try_push`.
            unsafe { Some(self.data[self.len].assume_init_read()) }
        }
    }

    /// Nombre d'éléments actuellement présents
    #[inline(always)]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Vrai si le vecteur ne contient aucun élément
    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Vrai si le vecteur a atteint sa capacité maximale `N`
    #[inline(always)]
    pub const fn is_full(&self) -> bool {
        self.len >= N
    }

    /// Capacité maximale allouée statiquement
    #[inline(always)]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Réinitialise la longueur à zéro en détruisant tous les éléments existants
    pub fn clear(&mut self) {
        while self.pop().is_some() {}
    }

    /// Vue sous forme de tranche immuable
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        // SAFETY: Les `self.len` premiers éléments sont initialisés.
        unsafe {
            let slice_ptr = self.data.as_ptr() as *const T;
            core::slice::from_raw_parts(slice_ptr, self.len)
        }
    }

    /// Vue sous forme de tranche mutable
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        // SAFETY: Les `self.len` premiers éléments sont initialisés.
        unsafe {
            let slice_ptr = self.data.as_mut_ptr() as *mut T;
            core::slice::from_raw_parts_mut(slice_ptr, self.len)
        }
    }
}

impl<T, const N: usize> Default for FixedVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Deref for FixedVec<T, N> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T, const N: usize> DerefMut for FixedVec<T, N> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl<T, const N: usize> Drop for FixedVec<T, N> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<T: Clone, const N: usize> Clone for FixedVec<T, N> {
    fn clone(&self) -> Self {
        let mut new_vec = Self::new();
        for item in self.as_slice() {
            let _ = new_vec.try_push(item.clone());
        }
        new_vec
    }
}
