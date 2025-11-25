//! Ring - Lock-free ring buffer for fusion rings
//!
//! Uses atomic head/tail pointers for wait-free single producer/single consumer

use core::sync::atomic::{AtomicU64, Ordering};
use super::slot::Slot;
use alloc::boxed::Box;

/// Default ring size (256 slots)
pub const DEFAULT_RING_SIZE: usize = 256;

/// Fusion ring buffer
pub struct Ring {
    /// Slots array (must be power of 2 size)
    slots: &'static [Slot],
    
    /// Head index (read position)
    head: AtomicU64,
    
    /// Tail index (write position)
    tail: AtomicU64,
    
    /// Capacity mask (size - 1)
    mask: u64,
}

unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    /// Create new ring with specified capacity (allocates slots)
    pub fn new(capacity: usize) -> &'static Self {
        assert!(capacity.is_power_of_two(), "Ring size must be power of 2");
        
        // Allocate slots (stub - would use proper allocator)
        use alloc::vec::Vec;
        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(Slot::new());
        }
        let slots = slots.leak(); // Leak to get 'static lifetime
        
        // Allocate Ring structure
        let ring = Box::new(Ring {
            slots,
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            mask: (slots.len() - 1) as u64,
        });
        Box::leak(ring)
    }
    
    /// Create new ring from preallocated slots
    pub unsafe fn from_slots(slots: &'static [Slot]) -> Self {
        assert!(slots.len().is_power_of_two(), "Ring size must be power of 2");
        
        Self {
            slots,
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            mask: (slots.len() - 1) as u64,
        }
    }
    
    /// Get capacity
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }
    
    /// Get current size
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        (tail.wrapping_sub(head) & self.mask) as usize
    }
    
    /// Check if ring is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    
    /// Check if ring is full
    pub fn is_full(&self) -> bool {
        self.len() == self.capacity()
    }
    
    /// Acquire write slot (returns None if full)
    pub fn acquire_write_slot(&self) -> Option<&Slot> {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        
        // Check if full
        if tail.wrapping_sub(head) >= self.mask + 1 {
            return None;
        }
        
        let index = (tail & self.mask) as usize;
        let slot = &self.slots[index];
        
        if slot.begin_write() {
            // Advance tail
            self.tail.store(tail.wrapping_add(1), Ordering::Release);
            Some(slot)
        } else {
            None
        }
    }
    
    /// Acquire read slot (returns None if empty)
    pub fn acquire_read_slot(&self) -> Option<&Slot> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        
        // Check if empty
        if head == tail {
            return None;
        }
        
        let index = (head & self.mask) as usize;
        let slot = &self.slots[index];
        
        if slot.begin_read() {
            // Advance head
            self.head.store(head.wrapping_add(1), Ordering::Release);
            Some(slot)
        } else {
            None
        }
    }
}
