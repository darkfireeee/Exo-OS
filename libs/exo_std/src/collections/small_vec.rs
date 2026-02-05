<<<<<<< Updated upstream
// libs/exo_std/src/collections/small_vec.rs
//! SmallVec : Vector optimisé avec stockage inline pour petites tailles
//!
//! SmallVec évite les allocations heap pour les vecteurs de petite taille
//! en les stockant directement dans la structure. Seulement si la taille
//! dépasse N éléments, une allocation (via BoundedVec) est utilisée.

use core::ops::{Deref, DerefMut, Index, IndexMut};
use core::ptr;
use core::mem::{self, MaybeUninit};
use core::slice;
use core::fmt;
use super::bounded_vec::CapacityError;

/// SmallVec avec stockage inline jusqu'à N éléments
///
/// # Exemple
/// ```no_run
/// use exo_std::collections::SmallVec;
///
/// // Peut stocker jusqu'à 8 éléments inline sans allocation
/// let mut vec: SmallVec<u32, 8> = SmallVec::new();
///
/// vec.push(1).unwrap();
/// vec.push(2).unwrap();
/// // Pas d'allocation tant que <= 8 éléments
/// ```
pub struct SmallVec<T, const N: usize> {
    /// Longueur actuelle
    len: usize,
    /// Stockage inline ou heap
=======
//! SmallVec : Vec qui utilise un buffer inline pour petites tailles
//!
//! Optimise les performances en évitant les allocations pour de petits vecteurs.

use core::mem::{MaybeUninit, ManuallyDrop};
use core::ptr;
use core::ops::{Deref, DerefMut};

extern crate alloc;
use alloc::vec::Vec;

/// SmallVec avec buffer inline de taille N
pub struct SmallVec<T, const N: usize> {
    len: usize,
>>>>>>> Stashed changes
    data: SmallVecData<T, N>,
}

union SmallVecData<T, const N: usize> {
<<<<<<< Updated upstream
    /// Stockage inline pour <= N éléments
    inline: MaybeUninit<[T; N]>,
    /// Pointeur vers heap si > N éléments
    /// Note: Dans un vrai système, on utiliserait un allocateur externe
    heap: *mut T,
}

impl<T, const N: usize> SmallVec<T, N> {
    /// Créé un nouveau SmallVec vide
    #[inline]
=======
    inline: ManuallyDrop<[MaybeUninit<T>; N]>,
    heap: ManuallyDrop<Vec<T>>,
}

impl<T, const N: usize> SmallVec<T, N> {
    /// Crée un nouveau SmallVec vide
>>>>>>> Stashed changes
    pub const fn new() -> Self {
        Self {
            len: 0,
            data: SmallVecData {
<<<<<<< Updated upstream
                inline: MaybeUninit::uninit(),
            },
        }
    }
    
    /// Crée un SmallVec avec une capacité spécifique
    ///
    /// Si capacity <= N, utilise inline storage
    /// Sinon, nécessite buffer externe (similaire à BoundedVec)
    ///
    /// # Safety
    /// Si capacity > N, `heap_buffer` doit pointer vers mémoire valide
    #[inline]
    pub unsafe fn with_capacity(capacity: usize, heap_buffer: *mut T) -> Self {
=======
                inline: ManuallyDrop::new(unsafe { MaybeUninit::uninit().assume_init() }),
            },
        }
    }

    /// Crée avec capacité
    pub fn with_capacity(capacity: usize) -> Self {
>>>>>>> Stashed changes
        if capacity <= N {
            Self::new()
        } else {
            Self {
                len: 0,
<<<<<<< Updated upstream
                data: SmallVecData { heap: heap_buffer },
            }
        }
    }
    
    /// Vérifie si utilise le stockage inline
    #[inline]
    pub const fn is_inline(&self) -> bool {
        self.len <= N
    }
    
    /// Ajoute un élément
    ///
    /// Retourne Err si la capacité maximale est atteinte
    #[inline]
    pub fn push(&mut self, value: T) -> Result<(), CapacityError> {
        if self.len < N {
            // Push inline
            unsafe {
                let ptr = self.data.inline.as_mut_ptr() as *mut T;
                ptr::write(ptr.add(self.len), value);
            }
            self.len += 1;
            Ok(())
        } else {
            // Pour l'instant, limite à N éléments
            // Dans une vraie impl, gérerait l'allocation heap
            Err(CapacityError)
        }
    }
    
    /// Retire et retourne le dernier élément
    #[inline]
=======
                data: SmallVecData {
                    heap: ManuallyDrop::new(Vec::with_capacity(capacity)),
                },
            }
        }
    }

    /// Ajoute un élément
    pub fn push(&mut self, value: T) {
        if self.len < N {
            // Inline
            unsafe {
                (*self.data.inline)[self.len] = MaybeUninit::new(value);
            }
            self.len += 1;
        } else if self.len == N {
            // Transition vers heap
            let mut vec = Vec::with_capacity(N * 2);
            unsafe {
                for i in 0..N {
                    let value = (*self.data.inline)[i].assume_init_read();
                    vec.push(value);
                }
            }
            vec.push(value);
            self.data = SmallVecData {
                heap: ManuallyDrop::new(vec),
            };
            self.len = N + 1;
        } else {
            // Heap
            unsafe {
                (*self.data.heap).push(value);
            }
            self.len += 1;
        }
    }

    /// Retire le dernier élément
>>>>>>> Stashed changes
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
<<<<<<< Updated upstream
        
        self.len -= 1;
        unsafe {
            if self.is_inline() {
                let ptr = self.data.inline.as_mut_ptr() as *mut T;
                Some(ptr::read(ptr.add(self.len)))
            } else {
                Some(ptr::read(self.data.heap.add(self.len)))
            }
        }
    }
    
    /// Insère à l'index donné
    #[inline]
    pub fn insert(&mut self, index: usize, value: T) -> Result<(), CapacityError> {
        assert!(index <= self.len, "index out of bounds");
        
        if self.len >= N {
            return Err(CapacityError);
        }
        
        unsafe {
            let ptr = if self.is_inline() {
                self.data.inline.as_mut_ptr() as *mut T
            } else {
                self.data.heap
            };
            
            let insert_ptr = ptr.add(index);
            ptr::copy(insert_ptr, insert_ptr.add(1), self.len - index);
            ptr::write(insert_ptr, value);
        }
        self.len += 1;
        Ok(())
    }
    
    /// Retire l'élément à l'index donné
    #[inline]
    pub fn remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "index out of bounds");
        
        unsafe {
            let ptr = if self.is_inline() {
                self.data.inline.as_mut_ptr() as *mut T
            } else {
                self.data.heap
            };
            
            let remove_ptr = ptr.add(index);
            let value = ptr::read(remove_ptr);
            
            ptr::copy(remove_ptr.add(1), remove_ptr, self.len - index - 1);
            
            self.len -= 1;
            value
        }
    }
    
    /// Swap remove (plus rapide, ne préserve pas l'ordre)
    #[inline]
    pub fn swap_remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "index out of bounds");
        
        unsafe {
            let ptr = if self.is_inline() {
                self.data.inline.as_mut_ptr() as *mut T
            } else {
                self.data.heap
            };
            
            let remove_ptr = ptr.add(index);
            let value = ptr::read(remove_ptr);
            
            self.len -= 1;
            if index != self.len {
                ptr::copy(ptr.add(self.len), remove_ptr, 1);
            }
            
            value
        }
    }
    
    /// Efface tous les éléments
    #[inline]
    pub fn clear(&mut self) {
        unsafe {
            let ptr = if self.is_inline() {
                self.data.inline.as_mut_ptr() as *mut T
            } else {
                self.data.heap
            };
            
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(ptr, self.len));
            self.len = 0;
        }
    }
    
    /// Tronque à len éléments
    #[inline]
    pub fn truncate(&mut self, len: usize) {
        if len < self.len {
            unsafe {
                let ptr = if self.is_inline() {
                    self.data.inline.as_mut_ptr() as *mut T
                } else {
                    self.data.heap
                };
                
                let drop_ptr = ptr.add(len);
                let count = self.len - len;
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(drop_ptr, count));
                self.len = len;
            }
        }
    }
    
    /// Étend depuis une slice
    pub fn extend_from_slice(&mut self, other: &[T]) -> Result<(), CapacityError>
    where
        T: Clone,
    {
        if self.len + other.len() > N {
            return Err(CapacityError);
        }
        
        for item in other {
            self.push(item.clone())?;
        }
        Ok(())
    }
    
    /// Conserve uniquement les éléments satisfaisant le prédicat
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&T) -> bool,
    {
        let mut i = 0;
        while i < self.len {
            if !f(&self[i]) {
                self.remove(i);
            } else {
                i += 1;
            }
        }
    }
    
    /// Déduplique les éléments consécutifs égaux
    pub fn dedup(&mut self)
    where
        T: PartialEq,
    {
        let mut i = 1;
        while i < self.len {
            if self[i] == self[i - 1] {
                self.remove(i);
            } else {
                i += 1;
            }
        }
    }
    
    /// Accès à l'élément
    #[inline]
    pub fn get(&self, index: usize) -> Option<&T> {
        if index < self.len {
            Some(&self.as_slice()[index])
        } else {
            None
        }
    }
    
    /// Accès mutable
    #[inline]
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index < self.len {
            Some(&mut self.as_mut_slice()[index])
        } else {
            None
        }
    }
    
    /// Premier élément
    #[inline]
    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }
    
    /// Dernier élément
    #[inline]
    pub fn last(&self) -> Option<&T> {
        if self.len > 0 {
            self.get(self.len - 1)
        } else {
            None
        }
    }
    
    /// Longueur
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }
    
    /// Vérifie si vide
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    
    /// Capacité (inline capacity)
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }
    
    /// Capacité restante
    #[inline]
    pub const fn remaining(&self) -> usize {
        N - self.len
    }
    
    /// Vérifie si plein
    #[inline]
    pub const fn is_full(&self) -> bool {
        self.len >= N
    }
    
    /// Convertit en slice
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        unsafe {
            let ptr = if self.is_inline() {
                self.data.inline.as_ptr() as *const T
            } else {
                self.data.heap as *const T
            };
            slice::from_raw_parts(ptr, self.len)
        }
    }
    
    /// Convertit en slice mutable
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe {
            let ptr = if self.is_inline() {
                self.data.inline.as_mut_ptr() as *mut T
            } else {
                self.data.heap
            };
            slice::from_raw_parts_mut(ptr, self.len)
=======

        self.len -= 1;

        if self.len < N {
            // Inline
            unsafe {
                let value = (*self.data.inline)[self.len].assume_init_read();
                Some(value)
            }
        } else {
            // Heap
            unsafe { (*self.data.heap).pop() }
        }
    }

    /// Retourne la longueur
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Vérifie si vide
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Capacité actuelle
    pub fn capacity(&self) -> usize {
        if self.len <= N {
            N
        } else {
            unsafe { (*self.data.heap).capacity() }
        }
    }

    /// Supprime tous les éléments
    pub fn clear(&mut self) {
        if self.len <= N {
            // Inline
            unsafe {
                for i in 0..self.len {
                    (*self.data.inline)[i].assume_init_drop();
                }
            }
        } else {
            // Heap
            unsafe {
                (*self.data.heap).clear();
            }
        }
        self.len = 0;
    }

    /// Accès par index
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }

        if self.len <= N {
            // Inline
            unsafe {
                Some((*self.data.inline)[index].assume_init_ref())
            }
        } else {
            // Heap
            unsafe { (*self.data.heap).get(index) }
        }
    }

    /// Accès mutable par index
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.len {
            return None;
        }

        if self.len <= N {
            // Inline
            unsafe {
                Some((*self.data.inline)[index].assume_init_mut())
            }
        } else {
            // Heap
            unsafe { (*self.data.heap).get_mut(index) }
        }
    }

    /// Convertit en slice
    pub fn as_slice(&self) -> &[T] {
        if self.len <= N {
            unsafe {
                core::slice::from_raw_parts(
                    (*self.data.inline).as_ptr() as *const T,
                    self.len,
                )
            }
        } else {
            unsafe { (*self.data.heap).as_slice() }
        }
    }

    /// Convertit en slice mutable
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.len <= N {
            unsafe {
                core::slice::from_raw_parts_mut(
                    (*self.data.inline).as_mut_ptr() as *mut T,
                    self.len,
                )
            }
        } else {
            unsafe { (*self.data.heap).as_mut_slice() }
>>>>>>> Stashed changes
        }
    }
}

<<<<<<< Updated upstream
impl<T, const N: usize> Deref for SmallVec<T, N> {
    type Target = [T];
    
    #[inline]
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, const N: usize> DerefMut for SmallVec<T, N> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, I: slice::SliceIndex<[T]>, const N: usize> Index<I> for SmallVec<T, N> {
    type Output = I::Output;
    
    #[inline]
    fn index(&self, index: I) -> &Self::Output {
        &self.as_slice()[index]
    }
}

impl<T, I: slice::SliceIndex<[T]>, const N: usize> IndexMut<I> for SmallVec<T, N> {
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        &mut self.as_mut_slice()[index]
    }
}

impl<T, const N: usize> Drop for SmallVec<T, N> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<T: fmt::Debug, const N: usize> fmt::Debug for SmallVec<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: Clone, const N: usize> Clone for SmallVec<T, N> {
    fn clone(&self) -> Self {
        let mut new = Self::new();
        for item in self.as_slice() {
            new.push(item.clone()).expect("clone failed");
        }
        new
=======
impl<T, const N: usize> Drop for SmallVec<T, N> {
    fn drop(&mut self) {
        self.clear();
        if self.capacity() > N {
            unsafe {
                ManuallyDrop::drop(&mut self.data.heap);
            }
        }
    }
}

impl<T, const N: usize> Deref for SmallVec<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, const N: usize> DerefMut for SmallVec<T, N> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
>>>>>>> Stashed changes
    }
}

impl<T, const N: usize> Default for SmallVec<T, N> {
<<<<<<< Updated upstream
    #[inline]
=======
>>>>>>> Stashed changes
    fn default() -> Self {
        Self::new()
    }
}

<<<<<<< Updated upstream
unsafe impl<T: Send, const N: usize> Send for SmallVec<T, N> {}
unsafe impl<T: Sync, const N: usize> Sync for SmallVec<T, N> {}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_small_vec_inline() {
        let mut vec: SmallVec<u32, 8> = SmallVec::new();
        
        assert!(vec.is_inline());
        assert!(vec.is_empty());
        
        vec.push(1).unwrap();
        vec.push(2).unwrap();
        vec.push(3).unwrap();
        
        assert_eq!(vec.len(), 3);
        assert_eq!(vec[0], 1);
        assert_eq!(vec[1], 2);
        assert_eq!(vec[2], 3);
        
        assert_eq!(vec.pop(), Some(3));
        assert_eq!(vec.len(), 2);
    }
    
    #[test]
    fn test_small_vec_capacity() {
        let mut vec: SmallVec<u32, 3> = SmallVec::new();
        
        vec.push(1).unwrap();
        vec.push(2).unwrap();
        vec.push(3).unwrap();
        
        assert!(vec.is_full());
        assert!(vec.push(4).is_err());
    }
    
    #[test]
    fn test_small_vec_swap_remove() {
        let mut vec: SmallVec<u32, 8> = SmallVec::new();
        vec.push(1).unwrap();
        vec.push(2).unwrap();
        vec.push(3).unwrap();
        vec.push(4).unwrap();
        
        let removed = vec.swap_remove(1);
        assert_eq!(removed, 2);
        assert_eq!(vec.as_slice(), &[1, 4, 3]);
=======
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_vec_inline() {
        let mut sv: SmallVec<i32, 4> = SmallVec::new();
        
        sv.push(1);
        sv.push(2);
        sv.push(3);
        
        assert_eq!(sv.len(), 3);
        assert_eq!(sv.get(0), Some(&1));
        assert_eq!(sv.get(1), Some(&2));
        assert_eq!(sv.pop(), Some(3));
        assert_eq!(sv.len(), 2);
    }

    #[test]
    fn test_small_vec_transition() {
        let mut sv: SmallVec<i32, 2> = SmallVec::new();
        
        sv.push(1);
        sv.push(2);
        sv.push(3); // Transition to heap
        sv.push(4);
        
        assert_eq!(sv.len(), 4);
        assert!(sv.capacity() > 2);
>>>>>>> Stashed changes
    }
}
