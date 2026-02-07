//! Utilitaires de performance et optimisations
//!
//! Fournit des primitives et utilitaires pour améliorer les performances.

use core::sync::atomic::{fence, Ordering, compiler_fence};

/// Hint au compilateur qu'une branche est probablement vraie
///
/// Optimise le code généré en indiquant que `cond` sera vraisemblablement vrai.
#[inline(always)]
pub const fn likely(cond: bool) -> bool {
    // Note: Utilise la primitive intrinsèque si disponible
    // Pour l'instant, retourne simplement la condition
    cond
}

/// Hint au compilateur qu'une branche est probablement fausse
///
/// Optimise le code généré en indiquant que `cond` sera vraisemblablement faux.
#[inline(always)]
pub const fn unlikely(cond: bool) -> bool {
    !likely(!cond)
}

/// Barrière mémoire complète optimisée
///
/// Force l'ordre des accès mémoire de manière portable.
#[inline(always)]
pub fn memory_barrier() {
    fence(Ordering::SeqCst);
}

/// Barrière de compilation (empêche le réordonnancement par le compilateur)
///
/// Plus légère qu'une barrière mémoire complète.
#[inline(always)]
pub fn compiler_barrier() {
    compiler_fence(Ordering::SeqCst);
}

/// Prefetch pour optimiser l'accès mémoire
///
/// Indique au CPU de charger `addr` dans le cache.
#[inline(always)]
pub unsafe fn prefetch_read<T>(addr: *const T) {
    #[cfg(target_arch = "x86_64")]
    {
        core::arch::x86_64::_mm_prefetch(addr as *const i8, core::arch::x86_64::_MM_HINT_T0);
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        // Sur les autres architectures, le hint est ignoré
        let _ = addr;
    }
}

/// Prefetch pour écriture
#[inline(always)]
pub unsafe fn prefetch_write<T>(addr: *mut T) {
    #[cfg(target_arch = "x86_64")]
    {
        core::arch::x86_64::_mm_prefetch(addr as *const i8, core::arch::x86_64::_MM_HINT_T0);
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = addr;
    }
}

/// Aligne une adresse vers le haut à l'alignement donné
///
/// # Exemple
/// ```
/// use exo_std::perf::align_up;
///
/// assert_eq!(align_up(10, 8), 16);
/// assert_eq!(align_up(16, 8), 16);
/// ```
#[inline(always)]
pub const fn align_up(addr: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two(), "alignment must be power of 2");
    (addr + align - 1) & !(align - 1)
}

/// Aligne une adresse vers le bas à l'alignement donné
#[inline(always)]
pub const fn align_down(addr: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two(), "alignment must be power of 2");
    addr & !(align - 1)
}

/// Vérifie si une adresse est alignée
#[inline(always)]
pub const fn is_aligned(addr: usize, align: usize) -> bool {
    debug_assert!(align.is_power_of_two(), "alignment must be power of 2");
    addr & (align - 1) == 0
}

/// Taille de ligne de cache typique (64 bytes sur x86_64)
pub const CACHE_LINE_SIZE: usize = 64;

/// Macro pour marquer un cold path (rarement exécuté)
#[cold]
#[inline(never)]
pub fn cold() {}

/// Structure alignée sur ligne de cache pour éviter le false sharing
///
/// # Exemple
/// ```no_run
/// use exo_std::perf::CacheAligned;
/// use core::sync::atomic::AtomicU64;
///
/// struct SharedCounters {
///     counter1: CacheAligned<AtomicU64>,
///     counter2: CacheAligned<AtomicU64>,
/// }
/// ```
#[repr(align(64))]
#[derive(Debug)]
pub struct CacheAligned<T> {
    value: T,
}

impl<T> CacheAligned<T> {
    /// Crée une nouvelle valeur alignée sur cache
    #[inline(always)]
    pub const fn new(value: T) -> Self {
        Self { value }
    }

    /// Obtient une référence à la valeur
    #[inline(always)]
    pub const fn get(&self) -> &T {
        &self.value
    }

    /// Obtient une référence mutable
    #[inline(always)]
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }

    /// Consomme et retourne la valeur interne
    #[inline(always)]
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T: Default> Default for CacheAligned<T> {
    #[inline]
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: Clone> Clone for CacheAligned<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self::new(self.value.clone())
    }
}

impl<T: Copy> Copy for CacheAligned<T> {}

impl<T> core::ops::Deref for CacheAligned<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &T {
        self.get()
    }
}

impl<T> core::ops::DerefMut for CacheAligned<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        self.get_mut()
    }
}

/// Compteur de cycles CPU (x86_64 uniquement)
///
/// Utilise RDTSC pour mesurer les cycles précis.
#[inline(always)]
pub fn read_cycle_counter() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_rdtsc()
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

/// Pause pour spinlock (réduit la consommation énergétique)
#[inline(always)]
pub fn spin_loop_hint() {
    core::hint::spin_loop();
}

/// Calcule la puissance de 2 suivante
///
/// # Exemple
/// ```
/// use exo_std::perf::next_power_of_two;
///
/// assert_eq!(next_power_of_two(10), 16);
/// assert_eq!(next_power_of_two(16), 16);
/// ```
#[inline(always)]
pub const fn next_power_of_two(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut p = 1;
    while p < n {
        p <<= 1;
    }
    p
}

/// Vérifie si un nombre est une puissance de 2
#[inline(always)]
pub const fn is_power_of_two(n: usize) -> bool {
    n != 0 && (n & (n - 1)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_align_up() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(7, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 8), 16);
    }

    #[test]
    fn test_align_down() {
        assert_eq!(align_down(0, 8), 0);
        assert_eq!(align_down(7, 8), 0);
        assert_eq!(align_down(8, 8), 8);
        assert_eq!(align_down(15, 8), 8);
        assert_eq!(align_down(16, 8), 16);
    }

    #[test]
    fn test_is_aligned() {
        assert!(is_aligned(0, 8));
        assert!(!is_aligned(1, 8));
        assert!(is_aligned(8, 8));
        assert!(is_aligned(16, 8));
        assert!(!is_aligned(17, 8));
    }

    #[test]
    fn test_next_power_of_two() {
        assert_eq!(next_power_of_two(0), 1);
        assert_eq!(next_power_of_two(1), 1);
        assert_eq!(next_power_of_two(2), 2);
        assert_eq!(next_power_of_two(3), 4);
        assert_eq!(next_power_of_two(10), 16);
        assert_eq!(next_power_of_two(16), 16);
        assert_eq!(next_power_of_two(17), 32);
    }

    #[test]
    fn test_is_power_of_two() {
        assert!(!is_power_of_two(0));
        assert!(is_power_of_two(1));
        assert!(is_power_of_two(2));
        assert!(!is_power_of_two(3));
        assert!(is_power_of_two(16));
        assert!(!is_power_of_two(17));
    }

    #[test]
    fn test_cache_aligned() {
        let aligned = CacheAligned::new(42u64);
        assert_eq!(*aligned, 42);

        let mut aligned = CacheAligned::new(100u32);
        *aligned.get_mut() = 200;
        assert_eq!(*aligned, 200);
    }
}
