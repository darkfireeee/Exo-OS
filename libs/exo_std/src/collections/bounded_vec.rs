// libs/exo_std/src/collections/bounded_vec.rs
//! Vector à capacité fixe sans réallocation
//!
//! BoundedVec est un vecteur de taille fixe qui ne peut pas croître au-delà
//! de sa capacité initiale. Idéal pour les environnements où les allocations
//! dynamiques doivent être évitées.

use core::ops::{Deref, DerefMut, Index, IndexMut, RangeBounds};
use core::ptr;
use core::slice::{self, SliceIndex};
use core::fmt;
use core::mem;

/// Erreur quand la capacité est dépassée
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacityError;

impl fmt::Display for CapacityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "capacity exceeded")
    }
}

/// Vector avec capacité maximale fixe (pas de réallocation)
///
/// # Exemple
/// ```no_run
/// use exo_std::collections::BoundedVec;
///
/// let mut buffer = [0u32; 10];
/// let mut vec = unsafe { BoundedVec::new(buffer.as_mut_ptr(), 10) };
///
/// vec.push(1).unwrap();
/// vec.push(2).unwrap();
/// assert_eq!(vec.len(), 2);
/// assert_eq!(vec[0], 1);
/// ```
pub struct BoundedVec<T> {
    buffer: *mut T,
    len: usize,
    capacity: usize,
}

impl<T> BoundedVec<T> {
    /// Crée un nouveau BoundedVec
    ///
    /// # Safety
    /// - `buffer` doit pointer vers une mémoire valide non-initialisée pour `capacity` éléments
    /// - `buffer` est possédé par le BoundedVec
    /// - La mémoire sera Drop-ée mais pas désallouée
    #[inline]
    pub const unsafe fn new(buffer: *mut T, capacity: usize) -> Self {
        Self {
            buffer,
            len: 0,
            capacity,
        }
    }
    
    /// Ajoute un élément à la fin
    ///
    /// Retourne Err si la capacité est atteinte.
    #[inline]
    pub fn push(&mut self, value: T) -> Result<(), CapacityError> {
        if self.len >= self.capacity {
            return Err(CapacityError);
        }
        
        unsafe {
            ptr::write(self.buffer.add(self.len), value);
        }
        self.len += 1;
        Ok(())
    }
    
    /// Tente d'ajouter un élément, retourne l'élément si échec
    #[inline]
    pub fn try_push(&mut self, value: T) -> Result<(), T> {
        if self.len >= self.capacity {
            return Err(value);
        }
        
        unsafe {
            ptr::write(self.buffer.add(self.len), value);
        }
        self.len += 1;
        Ok(())
    }
    
    /// Retire et retourne le dernier élément
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        
        self.len -= 1;
        unsafe {
            Some(ptr::read(self.buffer.add(self.len)))
        }
    }
    
    /// Insère un élément à l'index donné
    ///
    /// # Panics
    /// Panique si index > len
    #[inline]
    pub fn insert(&mut self, index: usize, value: T) -> Result<(), CapacityError> {
        assert!(index <= self.len, "index out of bounds");
        
        if self.len >= self.capacity {
            return Err(CapacityError);
        }
        
        unsafe {
            let ptr = self.buffer.add(index);
            // Décale les éléments vers la droite
            ptr::copy(ptr, ptr.add(1), self.len - index);
            ptr::write(ptr, value);
        }
        self.len += 1;
        Ok(())
    }
    
    /// Retire l'élément à l'index donné
    ///
    /// # Panics
    /// Panique si index >= len
    #[inline]
    pub fn remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "index out of bounds");
        
        unsafe {
            let ptr = self.buffer.add(index);
            let value = ptr::read(ptr);
            
            // Décale les éléments vers la gauche
            ptr::copy(ptr.add(1), ptr, self.len - index - 1);
            
            self.len -= 1;
            value
        }
    }
    
    /// Retire et retourne l'élément à index en swappant avec le dernier
    ///
    /// Plus rapide que remove() mais ne préserve pas l'ordre.
    ///
    /// # Panics
    /// Panique si index >= len
    #[inline]
    pub fn swap_remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "index out of bounds");
        
        unsafe {
            let ptr = self.buffer.add(index);
            let value = ptr::read(ptr);
            
            self.len -= 1;
            if index != self.len {
                ptr::copy(self.buffer.add(self.len), ptr, 1);
            }
            
            value
        }
    }
    
    /// Efface tous les éléments
    #[inline]
    pub fn clear(&mut self) {
        while self.pop().is_some() {}
    }
    
    /// Tronque à la longueur donnée
    ///
    /// Si len >= self.len(), ne fait rien.
    #[inline]
    pub fn truncate(&mut self, len: usize) {
        if len < self.len {
            unsafe {
                let remaining = self.len - len;
                let start = self.buffer.add(len);
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(start, remaining));
                self.len = len;
            }
        }
    }
    
    /// Étend avec les éléments d'un itérateur
    ///
    /// Retourne Err si la capacité est dépassée.
    pub fn extend_from_slice(&mut self, other: &[T]) -> Result<(), CapacityError>
    where
        T: Clone,
    {
        if self.len + other.len() > self.capacity {
            return Err(CapacityError);
        }
        
        for item in other {
            unsafe {
                ptr::write(self.buffer.add(self.len), item.clone());
                self.len += 1;
            }
        }
        
        Ok(())
    }
    
    /// Étend avec un itérateur
    pub fn try_extend<I>(&mut self, iter: I) -> Result<(), CapacityError>
    where
        I: IntoIterator<Item = T>,
    {
        for item in iter {
            self.push(item)?;
        }
        Ok(())
    }
    
    /// Retire les éléments dans la range donnée
    ///
    /// Retourne un itérateur sur les éléments retirés.
    pub fn drain<R>(&mut self, range: R) -> Drain<'_, T>
    where
        R: RangeBounds<usize>,
    {
        use core::ops::Bound;

        let len = self.len; // Capturer len avant borrow

        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };

        let end = match range.end_bound() {
            Bound::Included(&n) => n + 1,
            Bound::Excluded(&n) => n,
            Bound::Unbounded => len,
        };

        assert!(start <= end && end <= len, "invalid range");

        Drain {
            vec: self,
            start,
            end,
            tail_start: end,
            tail_len: len - end,
        }
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
    
    /// Accède à l'élément à l'index donné
    #[inline]
    pub fn get(&self, index: usize) -> Option<&T> {
        if index < self.len {
            unsafe {
                Some(&*self.buffer.add(index))
            }
        } else {
            None
        }
    }
    
    /// Accès mutable à l'élément à l'index donné
    #[inline]
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index < self.len {
            unsafe {
                Some(&mut *self.buffer.add(index))
            }
        } else {
            None
        }
    }
    
    /// Premier élément
    #[inline]
    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }
    
    /// Premier élément mutable
    #[inline]
    pub fn first_mut(&mut self) -> Option<&mut T> {
        self.get_mut(0)
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
    
    /// Dernier élément mutable
    #[inline]
    pub fn last_mut(&mut self) -> Option<&mut T> {
        let len = self.len;
        if len > 0 {
            self.get_mut(len - 1)
        } else {
            None
        }
    }
    
    /// Divise en deux slices mutables à l'index donné
    #[inline]
    pub fn split_at_mut(&mut self, mid: usize) -> (&mut [T], &mut [T]) {
        self.as_mut_slice().split_at_mut(mid)
    }
    
    /// Longueur actuelle
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }
    
    /// Vérifie si vide
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    
    /// Capacité maximale
    #[inline]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
    
    /// Capacité restante
    #[inline]
    pub const fn remaining(&self) -> usize {
        self.capacity - self.len
    }
    
    /// Vérifie si plein
    #[inline]
    pub const fn is_full(&self) -> bool {
        self.len == self.capacity
    }
    
    /// Convertit en slice
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        unsafe {
            slice::from_raw_parts(self.buffer, self.len)
        }
    }
    
    /// Convertit en slice mutable
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe {
            slice::from_raw_parts_mut(self.buffer, self.len)
        }
    }
    
    /// Pointeur brut vers le buffer
    #[inline]
    pub const fn as_ptr(&self) -> *const T {
        self.buffer
    }
    
    /// Pointeur brut mutable vers le buffer
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.buffer
    }
}

/// Itérateur pour drain
pub struct Drain<'a, T> {
    vec: &'a mut BoundedVec<T>,
    start: usize,
    end: usize,
    tail_start: usize,
    tail_len: usize,
}

impl<'a, T> Iterator for Drain<'a, T> {
    type Item = T;
    
    fn next(&mut self) -> Option<T> {
        if self.start < self.end {
            unsafe {
                let value = ptr::read(self.vec.buffer.add(self.start));
                self.start += 1;
                Some(value)
            }
        } else {
            None
        }
    }
    
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.end - self.start;
        (len, Some(len))
    }
}

impl<'a, T> Drop for Drain<'a, T> {
    fn drop(&mut self) {
        // Consomme le reste
        while self.next().is_some() {}
        
        // Décale la queue
        unsafe {
            ptr::copy(
                self.vec.buffer.add(self.tail_start),
                self.vec.buffer.add(self.start),
                self.tail_len,
            );
            self.vec.len = self.start + self.tail_len;
        }
    }
}

impl<T> Deref for BoundedVec<T> {
    type Target = [T];
    
    #[inline]
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> DerefMut for BoundedVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, I: SliceIndex<[T]>> Index<I> for BoundedVec<T> {
    type Output = I::Output;
    
    #[inline]
    fn index(&self, index: I) -> &Self::Output {
        &self.as_slice()[index]
    }
}

impl<T, I: SliceIndex<[T]>> IndexMut<I> for BoundedVec<T> {
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        &mut self.as_mut_slice()[index]
    }
}

impl<T> Drop for BoundedVec<T> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<T: fmt::Debug> fmt::Debug for BoundedVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: Clone> Clone for BoundedVec<T> {
    /// Clone le BoundedVec
    ///
    /// # Panics
    ///
    /// Panique toujours car BoundedVec ne peut pas allouer son propre buffer.
    /// Pour cloner un BoundedVec, vous devez:
    /// 1. Allouer manuellement un nouveau buffer de même capacité
    /// 2. Créer un nouveau BoundedVec avec ce buffer
    /// 3. Copier les éléments un par un avec extend_from_slice ou push
    ///
    /// # Exemple
    ///
    /// ```ignore
    /// let mut backing1 = vec![0u32; 10];
    /// let mut bv1 = unsafe { BoundedVec::new(backing1.as_mut_ptr(), 10) };
    /// bv1.push(42).unwrap();
    ///
    /// // Pour cloner:
    /// let mut backing2 = vec![0u32; 10];
    /// let mut bv2 = unsafe { BoundedVec::new(backing2.as_mut_ptr(), 10) };
    /// bv2.extend_from_slice(bv1.as_slice()).unwrap();
    /// ```
    fn clone(&self) -> Self {
        panic!(
            "BoundedVec::clone cannot allocate its own buffer. \
             You must manually allocate a buffer and copy elements. \
             See documentation for the Clone trait implementation."
        )
    }
}

unsafe impl<T: Send> Send for BoundedVec<T> {}
unsafe impl<T: Sync> Sync for BoundedVec<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::vec;
    
    #[test]
    fn test_bounded_vec_basic() {
        let mut backing = vec![0u32; 10];
        let mut bv = unsafe { BoundedVec::new(backing.as_mut_ptr(), 10) };
        
        assert!(bv.is_empty());
        assert_eq!(bv.capacity(), 10);
        
        bv.push(1).unwrap();
        bv.push(2).unwrap();
        bv.push(3).unwrap();
        assert_eq!(bv.len(), 3);
        
        assert_eq!(bv[0], 1);
        assert_eq!(bv[1], 2);
        assert_eq!(bv[2], 3);
        
        assert_eq!(bv.pop(), Some(3));
        assert_eq!(bv.len(), 2);
    }
    
    #[test]
    fn test_bounded_vec_capacity() {
        let mut backing = vec![0u32; 3];
        let mut bv = unsafe { BoundedVec::new(backing.as_mut_ptr(), 3) };
        
        bv.push(1).unwrap();
        bv.push(2).unwrap();
        bv.push(3).unwrap();
        assert_eq!(bv.remaining(), 0);
        assert!(bv.is_full());
        
        assert!(bv.push(4).is_err());
    }
    
    #[test]
    fn test_swap_remove() {
        let mut backing = vec![0u32; 10];
        let mut bv = unsafe { BoundedVec::new(backing.as_mut_ptr(), 10) };
        
        bv.push(1).unwrap();
        bv.push(2).unwrap();
        bv.push(3).unwrap();
        bv.push(4).unwrap();
        
        let removed = bv.swap_remove(1);
        assert_eq!(removed, 2);
        assert_eq!(bv.as_slice(), &[1, 4, 3]);
    }
}