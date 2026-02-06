// libs/exo_std/src/collections/ring_buffer.rs
//! Ring buffer (circular buffer) lock-free optimisé
//!
//! Implémentation SPSC (Single Producer Single Consumer) hautement optimisée
//! avec support de capacité en puissance de 2 pour masquage rapide.

use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Ring buffer lock-free SPSC (Single Producer Single Consumer)
///
/// Utilise des indices wrapping et un masque pour opérations O(1).
/// Nécessite une capacité en puissance de 2.
///
/// # Exemple
/// ```no_run
/// use exo_std::collections::RingBuffer;
///
/// let mut backing = vec![0u32; 8];
/// let rb = unsafe { RingBuffer::new(backing.as_mut_ptr(), 8) };
///
/// rb.push(1).unwrap();
/// rb.push(2).unwrap();
///
/// assert_eq!(rb.pop(), Some(1));
/// assert_eq!(rb.pop(), Some(2));
/// ```
pub struct RingBuffer<T> {
    buffer: *mut T,
    capacity: usize,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<T> RingBuffer<T> {
    /// Crée un nouveau ring buffer
    ///
    /// # Safety
    /// - `buffer` doit pointer vers une mémoire valide non-initialisée pour `capacity` éléments
    /// - `capacity` doit être une puissance de 2
    /// - Le buffer est possédé par RingBuffer et sera drop
    /// - Le buffer ne doit pas être désalloué manuellement
    ///
    /// # Panics
    /// Panique si capacity n'est pas une puissance de 2 ou est 0
    pub unsafe fn new(buffer: *mut T, capacity: usize) -> Self {
        assert!(capacity > 0 && capacity.is_power_of_two(),
            "Ring buffer capacity must be a power of 2");

        Self {
            buffer,
            capacity,
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Pousse un élément (producteur)
    ///
    /// Retourne Err(value) si le buffer est plein.
    /// Cette opération est lock-free pour un seul producteur.
    #[inline]
    pub fn push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        if head.wrapping_sub(tail) >= self.capacity {
            return Err(value);
        }

        unsafe {
            ptr::write(self.buffer.add(head & self.mask), value);
        }

        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Retire un élément (consommateur)
    ///
    /// Retourne None si le buffer est vide.
    /// Cette opération est lock-free pour un seul consommateur.
    #[inline]
    pub fn pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            return None;
        }

        let value = unsafe {
            ptr::read(self.buffer.add(tail & self.mask))
        };

        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    /// Nombre d'éléments actuellement dans le buffer
    #[inline]
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail)
    }

    /// Vérifie si le buffer est vide
    #[inline]
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }

    /// Vérifie si le buffer est plein
    #[inline]
    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail) >= self.capacity
    }

    /// Retourne la capacité maximale
    #[inline]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Retourne le nombre d'emplacements disponibles
    #[inline]
    pub fn remaining(&self) -> usize {
        self.capacity - self.len()
    }
}

unsafe impl<T: Send> Send for RingBuffer<T> {}
unsafe impl<T: Send> Sync for RingBuffer<T> {}

impl<T> Drop for RingBuffer<T> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::vec;

    #[test]
    fn test_ring_buffer_basic() {
        let mut backing = vec![0u32; 8];
        let rb = unsafe { RingBuffer::new(backing.as_mut_ptr(), 8) };

        assert!(rb.is_empty());
        assert!(!rb.is_full());
        assert_eq!(rb.len(), 0);

        rb.push(1).unwrap();
        rb.push(2).unwrap();
        assert_eq!(rb.len(), 2);
        assert_eq!(rb.remaining(), 6);

        assert_eq!(rb.pop(), Some(1));
        assert_eq!(rb.pop(), Some(2));
        assert!(rb.is_empty());
    }

    #[test]
    fn test_ring_buffer_wrap() {
        let mut backing = vec![0u32; 4];
        let rb = unsafe { RingBuffer::new(backing.as_mut_ptr(), 4) };

        rb.push(1).unwrap();
        rb.push(2).unwrap();
        rb.push(3).unwrap();
        assert!(rb.is_full());

        assert!(rb.push(4).is_err());

        assert_eq!(rb.pop(), Some(1));
        rb.push(4).unwrap();

        assert_eq!(rb.pop(), Some(2));
        assert_eq!(rb.pop(), Some(3));
        assert_eq!(rb.pop(), Some(4));
        assert!(rb.is_empty());
    }

    #[test]
    fn test_ring_buffer_full_cycle() {
        let mut backing = vec![0u64; 8];
        let rb = unsafe { RingBuffer::new(backing.as_mut_ptr(), 8) };

        for i in 0..100 {
            while let Err(_) = rb.push(i) {
                rb.pop();
            }
        }

        let mut count = 0;
        while rb.pop().is_some() {
            count += 1;
        }
        assert!(count <= 8);
    }

    #[test]
    #[should_panic(expected = "power of 2")]
    fn test_non_power_of_two() {
        let mut backing = vec![0u32; 5];
        unsafe { RingBuffer::new(backing.as_mut_ptr(), 5) };
    }
}
