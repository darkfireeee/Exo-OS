//! Lock-Free Ring Buffer Queue - Zero-Mutex Scheduler Pick
//!
//! Replaces Mutex<VecDeque> with atomic ring buffer
//! Target: < 50 cycles per pick_next (vs ~150 cycles with 3 mutexes)
//!
//! Architecture:
//! - Fixed-size ring buffer (256 slots = 1 cache line per queue)
//! - Atomic head/tail pointers (fetch_add)
//! - No allocations, cache-friendly
//! - SPSC optimized (Single Producer, Single Consumer)

use core::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use core::mem::MaybeUninit;

/// Lock-free ring buffer capacity (must be power of 2)
const QUEUE_CAPACITY: usize = 256;
const QUEUE_MASK: usize = QUEUE_CAPACITY - 1;

/// Cache-aligned lock-free queue (64 bytes = 1 cache line)
#[repr(align(64))]
pub struct LockFreeQueue {
    /// Head index (consumer reads here)
    head: AtomicUsize,
    
    /// Padding to separate head/tail on different cache lines
    _pad1: [u8; 56],
    
    /// Tail index (producer writes here)
    tail: AtomicUsize,
    
    /// Padding
    _pad2: [u8; 56],
    
    /// Ring buffer storage (thread IDs)
    buffer: [AtomicU16; QUEUE_CAPACITY],
}

impl LockFreeQueue {
    /// Create new empty queue
    pub const fn new() -> Self {
        const INIT: AtomicU16 = AtomicU16::new(0);
        Self {
            head: AtomicUsize::new(0),
            _pad1: [0; 56],
            tail: AtomicUsize::new(0),
            _pad2: [0; 56],
            buffer: [INIT; QUEUE_CAPACITY],
        }
    }
    
    /// Enqueue thread ID (lock-free)
    /// Returns false if queue is full
    #[inline(always)]
    pub fn enqueue(&self, thread_id: usize) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        
        // Check if queue is full
        let next_tail = tail.wrapping_add(1) & QUEUE_MASK;
        if next_tail == (head & QUEUE_MASK) {
            return false; // Queue full
        }
        
        // Store thread ID
        self.buffer[tail & QUEUE_MASK].store(thread_id as u16, Ordering::Release);
        
        // Update tail pointer
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        
        true
    }
    
    /// Dequeue thread ID (lock-free)
    /// Returns None if queue is empty
    #[inline(always)]
    pub fn dequeue(&self) -> Option<usize> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        
        // Check if queue is empty
        if (head & QUEUE_MASK) == (tail & QUEUE_MASK) {
            return None;
        }
        
        // Load thread ID
        let tid = self.buffer[head & QUEUE_MASK].load(Ordering::Acquire) as usize;
        
        // Update head pointer
        self.head.store(head.wrapping_add(1), Ordering::Release);
        
        Some(tid)
    }
    
    /// Check if queue is empty (approximate)
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        (head & QUEUE_MASK) == (tail & QUEUE_MASK)
    }
    
    /// Get approximate queue length
    #[inline(always)]
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        tail.wrapping_sub(head) & QUEUE_MASK
    }
}

// Safety: AtomicU16/AtomicUsize are Sync
unsafe impl Sync for LockFreeQueue {}
unsafe impl Send for LockFreeQueue {}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_enqueue_dequeue() {
        let queue = LockFreeQueue::new();
        
        assert!(queue.enqueue(42));
        assert!(queue.enqueue(123));
        
        assert_eq!(queue.dequeue(), Some(42));
        assert_eq!(queue.dequeue(), Some(123));
        assert_eq!(queue.dequeue(), None);
    }
    
    #[test]
    fn test_wrap_around() {
        let queue = LockFreeQueue::new();
        
        // Fill queue
        for i in 0..255 {
            assert!(queue.enqueue(i));
        }
        
        // Should be full
        assert!(!queue.enqueue(999));
        
        // Drain and refill
        for i in 0..255 {
            assert_eq!(queue.dequeue(), Some(i));
        }
        
        // Should work after wrap
        assert!(queue.enqueue(1000));
        assert_eq!(queue.dequeue(), Some(1000));
    }
}
