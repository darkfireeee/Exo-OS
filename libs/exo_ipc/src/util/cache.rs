// libs/exo_ipc/src/util/cache.rs
//! Utilitaires pour l'optimisation cache

use core::mem;
use core::ops::{Deref, DerefMut};

/// Taille d'une cache line sur x86_64 (64 bytes)
pub const CACHE_LINE_SIZE: usize = 64;

/// Wrapper pour aligner une structure sur une cache-line
/// Évite le false sharing entre threads
#[repr(C, align(64))]
#[derive(Debug)]
pub struct CachePadded<T> {
    value: T,
}

impl<T> CachePadded<T> {
    /// Crée une nouvelle valeur alignée sur cache-line
    pub const fn new(value: T) -> Self {
        Self { value }
    }
    
    /// Récupère une référence à la valeur
    pub fn get(&self) -> &T {
        &self.value
    }
    
    /// Récupère une référence mutable
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }
    
    /// Consomme le wrapper et retourne la valeur
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;
    
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: Default> Default for CachePadded<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: Clone> Clone for CachePadded<T> {
    fn clone(&self) -> Self {
        Self::new(self.value.clone())
    }
}

/// Pad pour séparer deux champs et éviter le false sharing
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Padding<const N: usize> {
    _padding: [u8; N],
}

impl<const N: usize> Padding<N> {
    /// Crée un nouveau padding
    pub const fn new() -> Self {
        Self { _padding: [0; N] }
    }
}

impl<const N: usize> Default for Padding<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Calcule le padding nécessaire pour aligner sur cache-line
pub const fn cache_line_padding<T>() -> usize {
    let size = mem::size_of::<T>();
    let remainder = size % CACHE_LINE_SIZE;
    if remainder == 0 {
        0
    } else {
        CACHE_LINE_SIZE - remainder
    }
}

/// Macro pour créer une structure avec padding automatique
#[macro_export]
macro_rules! cache_padded_struct {
    (
        $(#[$attr:meta])*
        pub struct $name:ident {
            $(pub $field:ident: $field_ty:ty),* $(,)?
        }
    ) => {
        #[repr(C, align(64))]
        $(#[$attr])*
        pub struct $name {
            $(pub $field: $field_ty,)*
            _padding: [u8; $crate::util::cache::cache_line_padding::<Self>()],
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicUsize;
    
    #[test]
    fn test_cache_padded_alignment() {
        let padded = CachePadded::new(42u64);
        let ptr = &padded as *const _ as usize;
        assert_eq!(ptr % CACHE_LINE_SIZE, 0, "Doit être aligné sur cache-line");
    }
    
    #[test]
    fn test_cache_padded_size() {
        assert_eq!(
            mem::size_of::<CachePadded<AtomicUsize>>(),
            CACHE_LINE_SIZE,
            "CachePadded doit faire exactement une cache-line"
        );
    }
    
    #[test]
    fn test_padding_calculation() {
        assert_eq!(cache_line_padding::<u8>(), 63);
        assert_eq!(cache_line_padding::<u64>(), 56);
        assert_eq!(cache_line_padding::<[u8; 64]>(), 0);
        assert_eq!(cache_line_padding::<[u8; 65]>(), 63);
    }
}
