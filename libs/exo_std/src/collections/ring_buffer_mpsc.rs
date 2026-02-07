//! RingBuffer MPSC (Multi-Producer Single-Consumer)
//!
//! Implémentation lock-free optimisée pour plusieurs producteurs et un seul consommateur.

use core::ptr;
use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};

/// Ring buffer MPSC avec verrous optimisés
///
/// Utilise un spinlock léger côté producteur et des atomics côté consommateur.
///
/// # Exemple
/// ```no_run
/// use exo_std::collections::RingBufferMpsc;
///
/// let mut backing = vec![0u32; 8];
/// let rb = unsafe { RingBufferMpsc::new(backing.as_mut_ptr(), 8) };
///
/// // Plusieurs producteurs peuvent pousser en parallèle
/// rb.push(1).unwrap();
/// rb.push(2).unwrap();
///
/// // Un seul consommateur retire les éléments
/// assert_eq!(rb.pop(), Some(1));
/// assert_eq!(rb.pop(), Some(2));
/// ```
pub struct RingBufferMpsc<T> {
    buffer: *mut T,
    capacity: usize,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    producer_lock: AtomicBool,
}

impl<T> RingBufferMpsc<T> {
    /// Crée un nouveau ring buffer MPSC
    ///
    /// # Safety
    /// - `buffer` doit pointer vers une mémoire valide non-initialisée pour `capacity` éléments
    /// - `capacity` doit être une puissance de 2
    /// - Le buffer est possédé par RingBufferMpsc et sera drop
    ///
    /// # Panics
    /// Panique si capacity n'est pas une puissance de 2 ou est 0
    pub unsafe fn new(buffer: *mut T, capacity: usize) -> Self {
        assert!(capacity > 0 && capacity.is_power_of_two(),
            "MPSC ring buffer capacity must be a power of 2");

        Self {
            buffer,
            capacity,
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            producer_lock: AtomicBool::new(false),
        }
    }

    /// Pousse un élément (producteurs multiples)
    ///
    /// Utilise un spinlock léger pour coordonner les producteurs.
    /// Retourne Err(value) si le buffer est plein.
    #[inline]
    pub fn push(&self, value: T) -> Result<(), T> {
        // Acquiert le verrou producteur
        while self.producer_lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }

        // Section critique
        let result = self.push_locked(value);

        // Libère le verrou
        self.producer_lock.store(false, Ordering::Release);

        result
    }

    #[inline]
    fn push_locked(&self, value: T) -> Result<(), T> {
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

    /// Retire un élément (consommateur unique)
    ///
    /// Opération lock-free pour le consommateur unique.
    /// Retourne None si le buffer est vide.
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

unsafe impl<T: Send> Send for RingBufferMpsc<T> {}
unsafe impl<T: Send> Sync for RingBufferMpsc<T> {}

impl<T> Drop for RingBufferMpsc<T> {
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
    fn test_mpsc_basic() {
        let mut backing = vec![0u32; 8];
        let rb = unsafe { RingBufferMpsc::new(backing.as_mut_ptr(), 8) };

        assert!(rb.is_empty());
        rb.push(1).unwrap();
        rb.push(2).unwrap();

        assert_eq!(rb.pop(), Some(1));
        assert_eq!(rb.pop(), Some(2));
        assert!(rb.is_empty());
    }

    #[test]
    fn test_mpsc_full() {
        let mut backing = vec![0u32; 4];
        let rb = unsafe { RingBufferMpsc::new(backing.as_mut_ptr(), 4) };

        rb.push(1).unwrap();
        rb.push(2).unwrap();
        rb.push(3).unwrap();
        assert!(rb.is_full());
        assert!(rb.push(4).is_err());

        assert_eq!(rb.pop(), Some(1));
        rb.push(4).unwrap();
        assert_eq!(rb.len(), 3);
    }

    #[test]
    fn test_mpsc_multiple_producers() {
        let mut backing = vec![0u64; 64];
        let rb = unsafe { RingBufferMpsc::new(backing.as_mut_ptr(), 64) };

        // Simule plusieurs producteurs (en séquentiel pour le test)
        for i in 0..32 {
            rb.push(i).unwrap();
        }

        assert_eq!(rb.len(), 32);

        for i in 0..32 {
            assert_eq!(rb.pop(), Some(i));
        }
    }
}
