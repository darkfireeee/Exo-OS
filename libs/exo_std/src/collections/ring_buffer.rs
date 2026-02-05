<<<<<<< Updated upstream
// libs/exo_std/src/collections/ring_buffer.rs
//! Circular buffer (ring buffer) with fixed capacity

use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Lock-free ring buffer (SPSC: Single Producer Single Consumer)
pub struct RingBuffer<T> {
    buffer: *mut T,
    capacity: usize,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<T> RingBuffer<T> {
    /// Create new ring buffer
    ///
    /// # Safety
    /// - `buffer` must point to valid memory for `capacity` elements
    /// - `capacity` must be power of 2
    /// - Buffer is owned by RingBuffer and will be dropped
    pub unsafe fn new(buffer: *mut T, capacity: usize) -> Self {
        assert!(capacity > 0 && capacity.is_power_of_two());
        
        Self {
            buffer,
            capacity,
            mask: capacity - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }
    
    /// Push element (producer)
    ///
    /// Returns Err if buffer is full
    pub fn push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        
        // Check if full: head is catching up to tail (wrapped around)
        // We use wrapping arithmetic to handle overflow correctly
        if head.wrapping_sub(tail) >= self.capacity {
            return Err(value);
        }
        
        // Write value at masked position
        unsafe {
            ptr::write(self.buffer.add(head & self.mask), value);
        }
        
        // Publish write - head can wrap around naturally
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }
    
    /// Pop element (consumer)
    ///
    /// Returns None if buffer is empty
    pub fn pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        
        // Check if empty: tail has caught up to head
        if tail == head {
            return None;
        }
        
        // Read value at masked position
        let value = unsafe {
            ptr::read(self.buffer.add(tail & self.mask))
        };
        
        // Publish read - tail can wrap around naturally
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(value)
    }
    
    /// Get number of elements
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        // Wrapping subtraction handles the circular nature correctly
        head.wrapping_sub(tail)
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }
    
    /// Check if full
    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail) >= self.capacity
    }
    
    /// Get capacity
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

unsafe impl<T: Send> Send for RingBuffer<T> {}
unsafe impl<T: Send> Sync for RingBuffer<T> {}

impl<T> Drop for RingBuffer<T> {
    fn drop(&mut self) {
        // Drop all remaining elements
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    
    #[test]
    fn test_ring_buffer() {
        let mut backing = vec![0u32; 8];
        let rb = unsafe { RingBuffer::new(backing.as_mut_ptr(), 8) };
        
        assert!(rb.is_empty());
        assert!(!rb.is_full());
        
        rb.push(1).unwrap();
        rb.push(2).unwrap();
        assert_eq!(rb.len(), 2);
        
        assert_eq!(rb.pop(), Some(1));
        assert_eq!(rb.pop(), Some(2));
        assert!(rb.is_empty());
    }
    
    #[test]
    fn test_ring_buffer_wraparound() {
        let mut backing = vec![0u32; 4];
        let rb = unsafe { RingBuffer::new(backing.as_mut_ptr(), 4) };
        
        // Fill to capacity - 1
        rb.push(1).unwrap();
        rb.push(2).unwrap();
        rb.push(3).unwrap();
        assert!(rb.is_full());
        
        // Can't push when full
        assert!(rb.push(4).is_err());
        
        // Pop one, can push again
        assert_eq!(rb.pop(), Some(1));
        rb.push(4).unwrap();
        
        assert_eq!(rb.pop(), Some(2));
        assert_eq!(rb.pop(), Some(3));
        assert_eq!(rb.pop(), Some(4));
        assert!(rb.is_empty());
=======
//! Ring buffer (circular buffer) optimisé pour no_std
//!
//! Plusieurs variantes : SPSC, MPSC, MPMC

use core::sync::atomic::{AtomicUsize, Ordering};
use core::cell::UnsafeCell;

/// Ring buffer générique
pub struct RingBuffer<T> {
    buffer: Box<[Option<T>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    capacity: usize,
}

impl<T> RingBuffer<T> {
    /// Crée un nouveau ring buffer
    pub fn new(capacity: usize) -> Self {
        let mut buffer = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            buffer.push(None);
        }

        Self {
            buffer: buffer.into_boxed_slice(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            capacity,
        }
    }

    /// Retourne la capacité
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Tente d'enqueue une valeur
    pub fn try_push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Acquire);
        let next_head = (head + 1) % self.capacity;
        let tail = self.tail.load(Ordering::Acquire);

        if next_head == tail {
            return Err(value); // Full
        }

        unsafe {
            let slot = &self.buffer[head] as *const Option<T> as *mut Option<T>;
            *slot = Some(value);
        }

        self.head.store(next_head, Ordering::Release);
        Ok(())
    }

    /// Tente de dequeue une valeur
    pub fn try_pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            return None; // Empty
        }

        let value = unsafe {
            let slot = &self.buffer[tail] as *const Option<T> as *mut Option<T>;
            (*slot).take()
        };

        let next_tail = (tail + 1) % self.capacity;
        self.tail.store(next_tail, Ordering::Release);

        value
    }

    /// Vérifie si vide
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }

    /// Vérifie si plein
    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let next_head = (head + 1) % self.capacity;
        next_head == self.tail.load(Ordering::Acquire)
    }
}

/// Ring buffer SPSC (Single Producer Single Consumer)
pub struct RingBufferSPSC<T> {
    inner: RingBuffer<T>,
}

impl<T> RingBufferSPSC<T> {
    /// Crée un nouveau SPSC ring buffer
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: RingBuffer::new(capacity),
        }
    }

    /// Push (producteur)
    pub fn push(&self, value: T) -> Result<(), T> {
        self.inner.try_push(value)
    }

    /// Pop (consommateur)
    pub fn pop(&self) -> Option<T> {
        self.inner.try_pop()
    }

    /// Capacité
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
}

/// Ring buffer MPSC (Multi Producer Single Consumer)
pub struct RingBufferMPSC<T> {
    inner: RingBuffer<T>,
}

impl<T> RingBufferMPSC<T> {
    /// Crée un nouveau MPSC ring buffer
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: RingBuffer::new(capacity),
        }
    }

    /// Push (plusieurs producteurs)
    pub fn push(&self, value: T) -> Result<(), T> {
        self.inner.try_push(value)
    }

    /// Pop (un seul consommateur)
    pub fn pop(&self) -> Option<T> {
        self.inner.try_pop()
    }

    /// Capacité
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
}

/// Ring buffer MPMC (Multi Producer Multi Consumer)
pub struct RingBufferMPMC<T> {
    inner: RingBuffer<T>,
}

impl<T> RingBufferMPMC<T> {
    /// Crée un nouveau MPMC ring buffer
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: RingBuffer::new(capacity),
        }
    }

    /// Push (plusieurs producteurs)
    pub fn push(&self, value: T) -> Result<(), T> {
        self.inner.try_push(value)
    }

    /// Pop (plusieurs consommateurs)
    pub fn pop(&self) -> Option<T> {
        self.inner.try_pop()
    }

    /// Capacité
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
}

// Implémentation basique de Box et Vec pour no_std
extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_spsc() {
        let rb = RingBufferSPSC::new(4);
        
        assert_eq!(rb.push(1), Ok(()));
        assert_eq!(rb.push(2), Ok(()));
        
        assert_eq!(rb.pop(), Some(1));
        assert_eq!(rb.pop(), Some(2));
        assert_eq!(rb.pop(), None);
    }

    #[test]
    fn test_ring_buffer_full() {
        let rb = RingBufferSPSC::new(3);
        
        assert_eq!(rb.push(1), Ok(()));
        assert_eq!(rb.push(2), Ok(()));
        assert!(rb.push(3).is_err()); // Full
>>>>>>> Stashed changes
    }
}
