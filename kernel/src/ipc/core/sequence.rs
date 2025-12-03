//! Sequence Numbers - Lock-free coordination for MPMC rings
//!
//! Sequences provide total ordering without locks. Each producer/consumer
//! tracks its position using atomic sequence numbers.
//!
//! ## Algorithm:
//! - Producers claim slots by advancing `producer_seq`
//! - Consumers claim slots by advancing `consumer_seq`
//! - Slots track their sequence to detect availability
//!
//! This achieves wait-free progress for single producer/consumer,
//! and lock-free progress for multiple.

use core::sync::atomic::{AtomicU64, Ordering};
use core::cell::UnsafeCell;

/// Cache line size for padding
const CACHE_LINE_SIZE: usize = 64;

/// Atomic sequence counter with cache line padding to prevent false sharing
#[repr(C, align(64))]
pub struct CacheLineCounter {
    value: AtomicU64,
    _pad: [u8; CACHE_LINE_SIZE - 8],
}

impl CacheLineCounter {
    pub const fn new(initial: u64) -> Self {
        Self {
            value: AtomicU64::new(initial),
            _pad: [0u8; CACHE_LINE_SIZE - 8],
        }
    }
    
    #[inline(always)]
    pub fn load(&self, order: Ordering) -> u64 {
        self.value.load(order)
    }
    
    #[inline(always)]
    pub fn store(&self, val: u64, order: Ordering) {
        self.value.store(val, order)
    }
    
    #[inline(always)]
    pub fn fetch_add(&self, val: u64, order: Ordering) -> u64 {
        self.value.fetch_add(val, order)
    }
    
    #[inline(always)]
    pub fn compare_exchange(
        &self,
        current: u64,
        new: u64,
        success: Ordering,
        failure: Ordering,
    ) -> Result<u64, u64> {
        self.value.compare_exchange(current, new, success, failure)
    }
    
    #[inline(always)]
    pub fn compare_exchange_weak(
        &self,
        current: u64,
        new: u64,
        success: Ordering,
        failure: Ordering,
    ) -> Result<u64, u64> {
        self.value.compare_exchange_weak(current, new, success, failure)
    }
}

/// Sequence number for slot coordination
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sequence(pub u64);

impl Sequence {
    pub const INITIAL: Self = Self(0);
    
    #[inline(always)]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    
    /// Convert sequence to slot index using mask
    #[inline(always)]
    pub const fn to_index(self, mask: u64) -> usize {
        (self.0 & mask) as usize
    }
    
    /// Get the expected sequence for a slot at this position
    /// This is used to detect if a slot is ready for the operation
    #[inline(always)]
    pub const fn expected_for_write(self, capacity: u64) -> u64 {
        self.0
    }
    
    #[inline(always)]
    pub const fn expected_for_read(self, capacity: u64) -> u64 {
        self.0 + 1
    }
    
    #[inline(always)]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
    
    #[inline(always)]
    pub const fn advance(self, n: u64) -> Self {
        Self(self.0.wrapping_add(n))
    }
    
    /// Calculate how many slots are available between two sequences
    #[inline(always)]
    pub const fn available_slots(producer: Self, consumer: Self, capacity: u64) -> u64 {
        // Number of filled slots
        producer.0.wrapping_sub(consumer.0)
    }
    
    /// Calculate free slots
    #[inline(always)]
    pub const fn free_slots(producer: Self, consumer: Self, capacity: u64) -> u64 {
        capacity - Self::available_slots(producer, consumer, capacity)
    }
}

/// Group of sequence counters for coordinating multiple producers/consumers
#[repr(C)]
pub struct SequenceGroup {
    /// Producer claim sequence (next slot to claim)
    pub producer_claim: CacheLineCounter,
    
    /// Producer commit sequence (last committed slot)
    pub producer_commit: CacheLineCounter,
    
    /// Consumer claim sequence (next slot to claim)
    pub consumer_claim: CacheLineCounter,
    
    /// Consumer commit sequence (last committed slot)  
    pub consumer_commit: CacheLineCounter,
    
    /// Cached producer commit (reduces cache line bouncing)
    cached_producer_commit: CacheLineCounter,
    
    /// Cached consumer commit (reduces cache line bouncing)
    cached_consumer_commit: CacheLineCounter,
}

impl SequenceGroup {
    pub const fn new() -> Self {
        Self {
            producer_claim: CacheLineCounter::new(0),
            producer_commit: CacheLineCounter::new(0),
            consumer_claim: CacheLineCounter::new(0),
            consumer_commit: CacheLineCounter::new(0),
            cached_producer_commit: CacheLineCounter::new(0),
            cached_consumer_commit: CacheLineCounter::new(0),
        }
    }
    
    /// Try to claim a slot for producing
    /// Returns the sequence number if successful
    #[inline]
    pub fn try_claim_produce(&self, capacity: u64) -> Option<Sequence> {
        let mut claimed = self.producer_claim.load(Ordering::Relaxed);
        
        loop {
            // Check if there's space
            let consumed = self.cached_consumer_commit.load(Ordering::Relaxed);
            
            if claimed.wrapping_sub(consumed) >= capacity {
                // Cache might be stale, refresh
                let fresh_consumed = self.consumer_commit.load(Ordering::Acquire);
                self.cached_consumer_commit.store(fresh_consumed, Ordering::Relaxed);
                
                if claimed.wrapping_sub(fresh_consumed) >= capacity {
                    return None; // Ring is full
                }
            }
            
            // Try to claim this slot
            match self.producer_claim.compare_exchange_weak(
                claimed,
                claimed.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(Sequence(claimed)),
                Err(new) => claimed = new, // Retry with new value
            }
        }
    }
    
    /// Commit a produced slot (make it visible to consumers)
    /// Must be called in sequence order
    #[inline]
    pub fn commit_produce(&self, seq: Sequence) {
        // Wait for previous producers to commit
        while self.producer_commit.load(Ordering::Acquire) != seq.0 {
            core::hint::spin_loop();
        }
        
        // Now we can commit
        self.producer_commit.store(seq.0.wrapping_add(1), Ordering::Release);
    }
    
    /// Try to claim a slot for consuming
    #[inline]
    pub fn try_claim_consume(&self) -> Option<Sequence> {
        let mut claimed = self.consumer_claim.load(Ordering::Relaxed);
        
        loop {
            // Check if there's data
            let produced = self.cached_producer_commit.load(Ordering::Relaxed);
            
            if claimed >= produced {
                // Cache might be stale, refresh
                let fresh_produced = self.producer_commit.load(Ordering::Acquire);
                self.cached_producer_commit.store(fresh_produced, Ordering::Relaxed);
                
                if claimed >= fresh_produced {
                    return None; // Ring is empty
                }
            }
            
            // Try to claim this slot
            match self.consumer_claim.compare_exchange_weak(
                claimed,
                claimed.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(Sequence(claimed)),
                Err(new) => claimed = new,
            }
        }
    }
    
    /// Commit a consumed slot
    #[inline]
    pub fn commit_consume(&self, seq: Sequence) {
        // Wait for previous consumers to commit
        while self.consumer_commit.load(Ordering::Acquire) != seq.0 {
            core::hint::spin_loop();
        }
        
        // Now we can commit
        self.consumer_commit.store(seq.0.wrapping_add(1), Ordering::Release);
    }
    
    /// Get number of items in the ring
    #[inline]
    pub fn len(&self) -> usize {
        let produced = self.producer_commit.load(Ordering::Acquire);
        let consumed = self.consumer_commit.load(Ordering::Acquire);
        produced.wrapping_sub(consumed) as usize
    }
    
    /// Check if empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Barrier for synchronizing multiple threads
/// Used for batch commits
pub struct SequenceBarrier {
    /// Sequence to wait for
    target: AtomicU64,
    
    /// Current sequence
    current: AtomicU64,
    
    /// Number of waiters
    waiters: AtomicU64,
}

impl SequenceBarrier {
    pub const fn new() -> Self {
        Self {
            target: AtomicU64::new(0),
            current: AtomicU64::new(0),
            waiters: AtomicU64::new(0),
        }
    }
    
    /// Wait until sequence reaches target
    #[inline]
    pub fn wait_for(&self, target: u64) {
        self.waiters.fetch_add(1, Ordering::Relaxed);
        
        while self.current.load(Ordering::Acquire) < target {
            core::hint::spin_loop();
        }
        
        self.waiters.fetch_sub(1, Ordering::Relaxed);
    }
    
    /// Signal that sequence has advanced
    #[inline]
    pub fn signal(&self, seq: u64) {
        self.current.store(seq, Ordering::Release);
    }
    
    /// Check if anyone is waiting
    #[inline]
    pub fn has_waiters(&self) -> bool {
        self.waiters.load(Ordering::Relaxed) > 0
    }
}
