//! RingBuffer MPMC (Multi-Producer Multi-Consumer)
//!
//! Implémentation avec verrous optimisés pour plusieurs producteurs et consommateurs.

use core::ptr;
use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use crate::sync::mutex::Backoff;

/// Ring buffer MPMC avec double spinlock optimisé
///
/// Utilise des spinlocks légers avec backoff exponentiel pour coordonner
/// plusieurs producteurs et consommateurs simultanément.
///
/// # Exemple
/// ```no_run
/// use exo_std::collections::RingBufferMpmc;
///
/// let mut backing = vec![0u32; 16];
/// let rb = unsafe { RingBufferMpmc::new(backing.as_mut_ptr(), 16) };
///
/// // Plusieurs producteurs ET consommateurs peuvent travailler en parallèle
/// rb.push(42).unwrap();
/// assert_eq!(rb.pop(), Some(42));
/// ```
pub struct RingBufferMpmc<T> {
    buffer: *mut T,
    capacity: usize,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    producer_lock: AtomicBool,
    consumer_lock: AtomicBool,
}

impl<T> RingBufferMpmc<T> {
    /// Crée un nouveau ring buffer MPMC
    ///
    /// # Safety
    /// - `buffer` doit pointer vers une mémoire valide non-initialisée pour `capacity` éléments
    /// - `capacity` doit être une puissance de 2
    /// - Le buffer est possédé par RingBufferMpmc et sera drop
    ///
    /// # Panics
    /// Panique si capacity n'est pas une puissance de 2 ou est 0
    pub unsafe fn new(buffer: *mut T, capacity: usize) -> Self {
        assert!(capacity > 0 && capacity.is_power_of_two(),
            "MPMC ring buffer capacity must be a power of 2");

        Self {
            buffer,
            capacity,
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            producer_lock: AtomicBool::new(false),
            consumer_lock: AtomicBool::new(false),
        }
    }

    /// Pousse un élément (producteurs multiples)
    ///
    /// Utilise un spinlock léger avec backoff pour coordonner les producteurs.
    /// Retourne Err(value) si le buffer est plein.
    #[inline]
    pub fn push(&self, value: T) -> Result<(), T> {
        let mut backoff = Backoff::new();

        // Acquiert le verrou producteur avec backoff
        loop {
            if self.producer_lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }

            backoff.spin();
            backoff.next();

            if backoff.should_yield() {
                #[cfg(not(feature = "test_mode"))]
                crate::syscall::thread::yield_now();
            }
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

    /// Retire un élément (consommateurs multiples)
    ///
    /// Utilise un spinlock léger avec backoff pour coordonner les consommateurs.
    /// Retourne None si le buffer est vide.
    #[inline]
    pub fn pop(&self) -> Option<T> {
        let mut backoff = Backoff::new();

        // Acquiert le verrou consommateur avec backoff
        loop {
            if self.consumer_lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                break;
            }

            backoff.spin();
            backoff.next();

            if backoff.should_yield() {
                #[cfg(not(feature = "test_mode"))]
                crate::syscall::thread::yield_now();
            }
        }

        // Section critique
        let result = self.pop_locked();

        // Libère le verrou
        self.consumer_lock.store(false, Ordering::Release);

        result
    }

    #[inline]
    fn pop_locked(&self) -> Option<T> {
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

    /// Tente de pousser sans attendre (non-bloquant)
    ///
    /// Retourne immédiatement Err(value) si le verrou est pris ou le buffer plein.
    #[inline]
    pub fn try_push(&self, value: T) -> Result<(), T> {
        if self.producer_lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(value);
        }

        let result = self.push_locked(value);
        self.producer_lock.store(false, Ordering::Release);

        result
    }

    /// Tente de retirer sans attendre (non-bloquant)
    ///
    /// Retourne None immédiatement si le verrou est pris ou le buffer vide.
    #[inline]
    pub fn try_pop(&self) -> Option<T> {
        if self.consumer_lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return None;
        }

        let result = self.pop_locked();
        self.consumer_lock.store(false, Ordering::Release);

        result
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

unsafe impl<T: Send> Send for RingBufferMpmc<T> {}
unsafe impl<T: Send> Sync for RingBufferMpmc<T> {}

impl<T> Drop for RingBufferMpmc<T> {
    fn drop(&mut self) {
        while self.pop_locked().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::vec;

    #[test]
    fn test_mpmc_basic() {
        let mut backing = vec![0u32; 8];
        let rb = unsafe { RingBufferMpmc::new(backing.as_mut_ptr(), 8) };

        assert!(rb.is_empty());
        rb.push(1).unwrap();
        rb.push(2).unwrap();

        assert_eq!(rb.pop(), Some(1));
        assert_eq!(rb.pop(), Some(2));
        assert!(rb.is_empty());
    }

    #[test]
    fn test_mpmc_full() {
        let mut backing = vec![0u32; 4];
        let rb = unsafe { RingBufferMpmc::new(backing.as_mut_ptr(), 4) };

        rb.push(1).unwrap();
        rb.push(2).unwrap();
        rb.push(3).unwrap();
        assert!(rb.is_full());
        assert!(rb.push(4).is_err());
    }

    #[test]
    fn test_mpmc_try_operations() {
        let mut backing = vec![0u32; 8];
        let rb = unsafe { RingBufferMpmc::new(backing.as_mut_ptr(), 8) };

        assert_eq!(rb.try_pop(), None);
        rb.try_push(42).unwrap();
        assert_eq!(rb.try_pop(), Some(42));
    }

    #[test]
    fn test_mpmc_sequential() {
        let mut backing = vec![0u64; 32];
        let rb = unsafe { RingBufferMpmc::new(backing.as_mut_ptr(), 32) };

        // Simule plusieurs producteurs/consommateurs en séquentiel
        for i in 0..20 {
            rb.push(i).unwrap();
        }

        for i in 0..10 {
            assert_eq!(rb.pop(), Some(i));
        }

        assert_eq!(rb.len(), 10);

        for i in 20..30 {
            rb.push(i).unwrap();
        }

        for i in 10..30 {
            assert_eq!(rb.pop(), Some(i));
        }

        assert!(rb.is_empty());
    }
}
