//! Ring - Lock-Free MPMC Ring Buffer
//!
//! High-performance multi-producer multi-consumer ring buffer using
//! cache-line aligned slots and sequence numbers for wait-free operation.
//!
//! ## Algorithm: Vyukov/LMAX-style bounded MPMC queue
//! - Per-slot sequence numbers prevent ABA problem
//! - CAS-based head/tail advancement
//! - Cache-line padding eliminates false sharing
//! - Wait-free in the common case
//!
//! ## Performance Targets:
//! - Single message send: ~150 cycles
//! - Batch send: ~50 cycles/msg amortized
//! - Contention: graceful degradation with CAS retry

use core::sync::atomic::{AtomicU64, Ordering};
use core::mem::MaybeUninit;
use core::ptr;
use alloc::boxed::Box;
use alloc::vec::Vec;
use super::slot::Slot;

/// Default ring size (must be power of 2)
pub const DEFAULT_RING_SIZE: usize = 256;

/// Cache line size for alignment
const CACHE_LINE: usize = 64;

/// Padded atomic for cache-line isolation
#[repr(C, align(64))]
struct CacheLinePadded<T> {
    value: T,
    _pad: [u8; CACHE_LINE - 8],
}

impl<T> CacheLinePadded<T> {
    const fn new(value: T) -> Self {
        Self {
            value,
            _pad: [0; CACHE_LINE - 8],
        }
    }
}

/// Ring slot with embedded sequence number
#[repr(C, align(64))]
pub struct RingSlot {
    /// Sequence number determines slot state:
    /// - seq == pos: slot is empty, ready for write
    /// - seq == pos + 1: slot has data, ready for read
    sequence: AtomicU64,
    /// Embedded data slot
    pub data: Slot,
}

impl RingSlot {
    /// Create new slot with initial sequence
    pub fn new(initial_seq: u64) -> Self {
        Self {
            sequence: AtomicU64::new(initial_seq),
            data: Slot::new(),
        }
    }
    
    /// Get current sequence
    #[inline]
    pub fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }
    
    /// Set sequence (after write complete)
    #[inline]
    pub fn set_sequence(&self, seq: u64) {
        self.sequence.store(seq, Ordering::Release);
    }
}

/// Lock-free MPMC Ring Buffer
///
/// Multiple producers and consumers can operate concurrently without locks.
/// Uses sequence numbers per slot to track state and prevent races.
pub struct Ring {
    /// Slots array (boxed to allow runtime sizing)
    slots: Box<[RingSlot]>,
    
    /// Capacity (always power of 2)
    capacity: usize,
    
    /// Mask for fast modulo (capacity - 1)
    mask: usize,
    
    /// Head position (producer/write cursor)
    /// Cache-line aligned to prevent false sharing
    head: CacheLinePadded<AtomicU64>,
    
    /// Tail position (consumer/read cursor)
    /// Cache-line aligned to prevent false sharing
    tail: CacheLinePadded<AtomicU64>,
    
    /// Statistics: total messages enqueued
    enqueue_count: AtomicU64,
    
    /// Statistics: total messages dequeued
    dequeue_count: AtomicU64,
    
    /// Statistics: CAS retries (contention indicator)
    cas_retries: AtomicU64,
}

// Safety: Ring uses only atomic operations, safe to share between threads
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    /// Create new MPMC ring with specified capacity
    ///
    /// # Arguments
    /// * `capacity` - Must be power of 2, minimum 2
    ///
    /// # Returns
    /// Static reference to ring (leaked for 'static lifetime)
    ///
    /// # Panics
    /// Panics if capacity is not power of 2 or less than 2
    pub fn new(capacity: usize) -> &'static Self {
        assert!(capacity.is_power_of_two(), "Ring capacity must be power of 2");
        assert!(capacity >= 2, "Ring capacity must be at least 2");
        
        // Allocate slots with initial sequence = slot index
        let slots: Vec<RingSlot> = (0..capacity)
            .map(|i| RingSlot::new(i as u64))
            .collect();
        
        let ring = Box::new(Self {
            slots: slots.into_boxed_slice(),
            capacity,
            mask: capacity - 1,
            head: CacheLinePadded::new(AtomicU64::new(0)),
            tail: CacheLinePadded::new(AtomicU64::new(0)),
            enqueue_count: AtomicU64::new(0),
            dequeue_count: AtomicU64::new(0),
            cas_retries: AtomicU64::new(0),
        });
        
        // Leak to get 'static lifetime
        // In production, use a proper allocator with cleanup
        Box::leak(ring)
    }
    
    /// Create ring from pre-existing slots (for shared memory scenarios)
    ///
    /// # Safety
    /// Caller must ensure slots are properly initialized with correct sequences
    pub unsafe fn from_slots(slots: &'static [Slot]) -> Self {
        let capacity = slots.len();
        assert!(capacity.is_power_of_two());
        
        // Wrap into RingSlots
        let ring_slots: Vec<RingSlot> = (0..capacity)
            .map(|i| RingSlot::new(i as u64))
            .collect();
        
        Self {
            slots: ring_slots.into_boxed_slice(),
            capacity,
            mask: capacity - 1,
            head: CacheLinePadded::new(AtomicU64::new(0)),
            tail: CacheLinePadded::new(AtomicU64::new(0)),
            enqueue_count: AtomicU64::new(0),
            dequeue_count: AtomicU64::new(0),
            cas_retries: AtomicU64::new(0),
        }
    }
    
    /// Get ring capacity
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    
    /// Get approximate current length
    ///
    /// Note: May be inaccurate under concurrent modification
    #[inline]
    pub fn len(&self) -> usize {
        let head = self.head.value.load(Ordering::Relaxed);
        let tail = self.tail.value.load(Ordering::Relaxed);
        (head.wrapping_sub(tail)) as usize
    }
    
    /// Check if ring appears empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        let head = self.head.value.load(Ordering::Relaxed);
        let tail = self.tail.value.load(Ordering::Relaxed);
        head == tail
    }
    
    /// Check if ring appears full
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len() >= self.capacity
    }
    
    /// Try to acquire a slot for writing (producer operation)
    ///
    /// # Returns
    /// - `Some(&RingSlot)` if slot acquired
    /// - `None` if ring is full
    ///
    /// # Algorithm
    /// 1. Load current head position
    /// 2. Check slot's sequence == head (slot is empty)
    /// 3. CAS head to head+1
    /// 4. On success, return slot reference
    /// 5. On failure (contention), retry
    #[inline]
    pub fn acquire_write_slot(&self) -> Option<&RingSlot> {
        let mut backoff = Backoff::new();
        
        loop {
            let head = self.head.value.load(Ordering::Relaxed);
            let slot_idx = (head as usize) & self.mask;
            let slot = &self.slots[slot_idx];
            
            let seq = slot.sequence.load(Ordering::Acquire);
            let diff = (seq as i64).wrapping_sub(head as i64);
            
            if diff == 0 {
                // Slot is ready for writing
                match self.head.value.compare_exchange_weak(
                    head,
                    head.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        self.enqueue_count.fetch_add(1, Ordering::Relaxed);
                        return Some(slot);
                    }
                    Err(_) => {
                        // CAS failed, another producer won
                        self.cas_retries.fetch_add(1, Ordering::Relaxed);
                        backoff.spin();
                        continue;
                    }
                }
            } else if diff < 0 {
                // Ring is full (slot still being read)
                return None;
            } else {
                // Slot was already claimed, reload head
                backoff.spin();
                continue;
            }
        }
    }
    
    /// Complete a write operation, making data visible to consumers
    ///
    /// # Arguments
    /// * `slot` - Slot previously acquired via `acquire_write_slot`
    ///
    /// Must be called after writing data to slot
    #[inline]
    pub fn complete_write(&self, slot: &RingSlot) {
        // Calculate what sequence should be for consumers
        // Sequence should be head position when we acquired + 1
        let slot_ptr = slot as *const RingSlot;
        let base_ptr = self.slots.as_ptr();
        let slot_idx = unsafe { slot_ptr.offset_from(base_ptr) } as usize;
        
        // Compute the position this slot corresponds to
        let pos = self.head.value.load(Ordering::Relaxed).wrapping_sub(1);
        
        // Set sequence to pos+1 to indicate data is ready
        slot.sequence.store(pos.wrapping_add(1), Ordering::Release);
    }
    
    /// Try to acquire a slot for reading (consumer operation)
    ///
    /// # Returns
    /// - `Some(&RingSlot)` if slot acquired
    /// - `None` if ring is empty
    #[inline]
    pub fn acquire_read_slot(&self) -> Option<&RingSlot> {
        let mut backoff = Backoff::new();
        
        loop {
            let tail = self.tail.value.load(Ordering::Relaxed);
            let slot_idx = (tail as usize) & self.mask;
            let slot = &self.slots[slot_idx];
            
            let seq = slot.sequence.load(Ordering::Acquire);
            let diff = (seq as i64).wrapping_sub((tail.wrapping_add(1)) as i64);
            
            if diff == 0 {
                // Slot has data ready
                match self.tail.value.compare_exchange_weak(
                    tail,
                    tail.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        self.dequeue_count.fetch_add(1, Ordering::Relaxed);
                        return Some(slot);
                    }
                    Err(_) => {
                        self.cas_retries.fetch_add(1, Ordering::Relaxed);
                        backoff.spin();
                        continue;
                    }
                }
            } else if diff < 0 {
                // Ring is empty
                return None;
            } else {
                // Another consumer took this slot
                backoff.spin();
                continue;
            }
        }
    }
    
    /// Complete a read operation, making slot available for producers
    ///
    /// # Arguments
    /// * `slot` - Slot previously acquired via `acquire_read_slot`
    #[inline]
    pub fn complete_read(&self, slot: &RingSlot) {
        let slot_ptr = slot as *const RingSlot;
        let base_ptr = self.slots.as_ptr();
        let slot_idx = unsafe { slot_ptr.offset_from(base_ptr) } as usize;
        
        // Set sequence to tail + capacity to make slot writable again
        let tail = self.tail.value.load(Ordering::Relaxed);
        slot.sequence.store(
            tail.wrapping_add(self.capacity as u64).wrapping_sub(1),
            Ordering::Release,
        );
    }
    
    /// Get ring statistics
    pub fn stats(&self) -> RingStats {
        RingStats {
            capacity: self.capacity,
            current_len: self.len(),
            total_enqueued: self.enqueue_count.load(Ordering::Relaxed),
            total_dequeued: self.dequeue_count.load(Ordering::Relaxed),
            cas_retries: self.cas_retries.load(Ordering::Relaxed),
        }
    }
}

/// Ring statistics for monitoring
#[derive(Debug, Clone, Copy)]
pub struct RingStats {
    /// Ring capacity
    pub capacity: usize,
    /// Current approximate length
    pub current_len: usize,
    /// Total messages enqueued
    pub total_enqueued: u64,
    /// Total messages dequeued
    pub total_dequeued: u64,
    /// CAS retry count (contention indicator)
    pub cas_retries: u64,
}

/// Exponential backoff for CAS retry
struct Backoff {
    step: u32,
}

impl Backoff {
    const MAX_STEP: u32 = 6;
    
    #[inline]
    fn new() -> Self {
        Self { step: 0 }
    }
    
    #[inline]
    fn spin(&mut self) {
        let spins = 1u32 << self.step.min(Self::MAX_STEP);
        for _ in 0..spins {
            core::hint::spin_loop();
        }
        if self.step < Self::MAX_STEP {
            self.step += 1;
        }
    }
}
