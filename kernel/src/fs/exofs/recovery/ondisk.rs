//! Helpers de sérialisation on-disk pour les structures `repr(C)` de recovery.
//!
//! Ces fonctions gardent les conversions byte-à-byte au même endroit et évitent
//! les `transmute*` dispersés dans les chemins de récupération.

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use core::mem::{size_of, MaybeUninit};

/// Copie une structure POD depuis un buffer on-disk déjà borné.
#[inline]
pub fn read_pod<T: Copy>(bytes: &[u8]) -> ExofsResult<T> {
    let size = size_of::<T>();
    if bytes.len() < size {
        return Err(ExofsError::CorruptedStructure);
    }

    let mut out = MaybeUninit::<T>::uninit();
    // SAFETY: `out` pointe vers `size_of::<T>()` octets valides et non initialisés.
    // `bytes` a été borné à au moins cette taille, et les régions ne se recouvrent pas.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out.as_mut_ptr() as *mut u8, size);
        Ok(out.assume_init())
    }
}

/// Sérialise une structure POD dans un tableau de taille fixe.
#[inline]
pub fn write_pod<T: Copy, const N: usize>(value: &T) -> [u8; N] {
    let mut out = [0u8; N];
    if size_of::<T>() != N {
        return out;
    }

    // SAFETY: `out` contient exactement `N == size_of::<T>()` octets valides.
    // La copie lit l'objet par octets sans créer de référence aliasée typée.
    unsafe {
        core::ptr::copy_nonoverlapping(value as *const T as *const u8, out.as_mut_ptr(), N);
    }
    out
}

/// Copie les `N` premiers octets d'une structure POD.
#[inline]
pub fn prefix_bytes<T: Copy, const N: usize>(value: &T) -> [u8; N] {
    let mut out = [0u8; N];
    if size_of::<T>() < N {
        return out;
    }

    // SAFETY: `value` a au moins `N` octets, vérifié ci-dessus. La destination
    // est un tableau local de `N` octets sans recouvrement.
    unsafe {
        core::ptr::copy_nonoverlapping(value as *const T as *const u8, out.as_mut_ptr(), N);
    }
    out
}

/// Initialise à zéro une structure POD utilisée uniquement comme tampon de test
/// ou structure disque composée de scalaires.
#[inline]
pub fn zero_pod<T: Copy>() -> T {
    let mut out = MaybeUninit::<T>::uninit();
    // SAFETY: les appelants de recovery utilisent ce helper pour des structs
    // `repr(C)` composées d'entiers et de tableaux d'octets.
    unsafe {
        core::ptr::write_bytes(out.as_mut_ptr() as *mut u8, 0, size_of::<T>());
        out.assume_init()
    }
}
