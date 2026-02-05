// libs/exo_std/src/sync/atomic.rs
//! Wrappers atomiques pour types simples
//!
//! Fournit AtomicCell pour types Copy qui ne sont pas nativement atomiques.

pub use core::sync::atomic::Ordering;
use core::cell::UnsafeCell;
use super::mutex::Mutex;

/// Cellule atomique pour types Copy
///
/// Contrairement aux types atomiques natifs, AtomicCell peut contenir
/// n'importe quel type Copy. Pour les petits types (≤ 8 bytes sur x64),
/// utilise des atomics natifs. Pour les plus grands, utilise un Mutex.
///
/// # Exemple
/// ```no_run
/// use exo_std::sync::AtomicCell;
///
/// let cell = AtomicCell::new((1, 2, 3));
/// cell.store((4, 5, 6));
/// let value = cell.load();
/// ```
pub struct AtomicCell<T: Copy> {
    inner: AtomicCellInner<T>,
}

enum AtomicCellInner<T: Copy> {
    /// Pour types ≤ 8 bytes: utilise UnsafeCell + atomics
    Small(UnsafeCell<T>),
    /// Pour types > 8 bytes: utilise un Mutex
    Large(Mutex<T>),
}

impl<T: Copy> AtomicCell<T> {
    /// Crée une nouvelle AtomicCell
    #[inline]
    pub const fn new(value: T) -> Self {
        if core::mem::size_of::<T>() <= 8 {
            Self {
                inner: AtomicCellInner::Small(UnsafeCell::new(value)),
            }
        } else {
            Self {
                inner: AtomicCellInner::Large(Mutex::new(value)),
            }
        }
    }
    
    /// Charge la valeur
    #[inline]
    pub fn load(&self) -> T {
        match &self.inner {
            AtomicCellInner::Small(cell) => {
                // Pour petits types, lit avec acquire semantics
                unsafe {
                    core::sync::atomic::fence(Ordering::Acquire);
                    core::ptr::read_volatile(cell.get())
                }
            }
            AtomicCellInner::Large(mutex) => {
                *mutex.lock().unwrap()
            }
        }
    }
    
    /// Stocke une valeur
    #[inline]
    pub fn store(&self, value: T) {
        match &self.inner {
            AtomicCellInner::Small(cell) => {
                // Pour petits types, écrit avec release semantics
                unsafe {
                    core::ptr::write_volatile(cell.get(), value);
                    core::sync::atomic::fence(Ordering::Release);
                }
            }
            AtomicCellInner::Large(mutex) => {
                *mutex.lock().unwrap() = value;
            }
        }
    }
    
    /// Swap atomique
    #[inline]
    pub fn swap(&self, value: T) -> T {
        match &self.inner {
            AtomicCellInner::Small(cell) => {
                unsafe {
                    let old = core::ptr::read_volatile(cell.get());
                    core::ptr::write_volatile(cell.get(), value);
                    core::sync::atomic::fence(Ordering::AcqRel);
                    old
                }
            }
            AtomicCellInner::Large(mutex) => {
                let mut guard = mutex.lock().unwrap();
                let old = *guard;
                *guard = value;
                old
            }
        }
    }
    
    /// Compare-and-swap
    #[inline]
    pub fn compare_and_swap(&self, current: T, new: T) -> T
    where
        T: PartialEq,
    {
        match &self.inner {
            AtomicCellInner::Small(cell) => {
                unsafe {
                    let old = core::ptr::read_volatile(cell.get());
                    if old == current {
                        core::ptr::write_volatile(cell.get(), new);
                    }
                    core::sync::atomic::fence(Ordering::AcqRel);
                    old
                }
            }
            AtomicCellInner::Large(mutex) => {
                let mut guard = mutex.lock().unwrap();
                let old = *guard;
                if old == current {
                    *guard = new;
                }
                old
            }
        }
    }
    
    /// Obtient une référence mutable (&mut self requis)
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        match &mut self.inner {
            AtomicCellInner::Small(cell) => cell.get_mut(),
            AtomicCellInner::Large(mutex) => mutex.get_mut(),
        }
    }
    
    /// Consomme et retourne la valeur
    #[inline]
    pub fn into_inner(self) -> T {
        match self.inner {
            AtomicCellInner::Small(cell) => cell.into_inner(),
            AtomicCellInner::Large(mutex) => mutex.into_inner(),
        }
    }
}

// Safety: AtomicCell est Send/Sync si T est Send
unsafe impl<T: Copy + Send> Send for AtomicCell<T> {}
unsafe impl<T: Copy + Send> Sync for AtomicCell<T> {}

impl<T: Copy + Default> Default for AtomicCell<T> {
    #[inline]
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: Copy> From<T> for AtomicCell<T> {
    #[inline]
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_atomic_cell_small() {
        let cell = AtomicCell::new(42u32);
        assert_eq!(cell.load(), 42);
        
        cell.store(100);
        assert_eq!(cell.load(), 100);
        
        let old = cell.swap(200);
        assert_eq!(old, 100);
        assert_eq!(cell.load(), 200);
    }
    
    #[test]
    fn test_atomic_cell_large() {
        let cell = AtomicCell::new([1u64; 4]);
        let val = cell.load();
        assert_eq!(val, [1; 4]);
        
        cell.store([2; 4]);
        assert_eq!(cell.load(), [2; 4]);
    }
}
