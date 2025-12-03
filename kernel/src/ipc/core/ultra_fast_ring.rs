//! Ultra-Fast Ring - Optimized for Minimum Latency
//!
//! This ring implementation is specifically optimized for the hot path
//! where latency is critical. Uses aggressive compiler hints, prefetching,
//! and relaxed memory ordering where safe.
//!
//! ## Optimizations:
//! 1. **Branch prediction hints** - likely/unlikely for hot paths
//! 2. **Cache prefetching** - Prefetch next slot during operation
//! 3. **Relaxed ordering** - Use weakest safe ordering
//! 4. **Inline everything** - No function call overhead
//! 5. **Power-of-2 masking** - Avoid modulo operation
//! 6. **Timestamped slots** - Latency tracking built-in
//!
//! ## Target: 80-100 cycles for inline send

use core::sync::atomic::{AtomicU64, AtomicU32, AtomicU8, Ordering, fence};
use core::ptr;
use core::hint;
use alloc::boxed::Box;
use alloc::vec::Vec;

use super::advanced::{
    TimestampedSlot, CoalesceController, CreditController, LaneStats,
    PriorityClass, prefetch_read, prefetch_write, prefetch_range,
    rdtsc, rdtscp, CACHE_LINE_SIZE, GLOBAL_PERF_COUNTERS,
};
use crate::memory::{MemoryResult, MemoryError};

// =============================================================================
// CONSTANTS
// =============================================================================

/// Default ring capacity
pub const DEFAULT_CAPACITY: usize = 256;

/// Maximum inline payload in fast slot (40 bytes)
pub const FAST_INLINE_MAX: usize = 40;

/// Spin limit before yielding
pub const SPIN_LIMIT: u32 = 64;

/// Spin limit before blocking
pub const BLOCK_THRESHOLD: u32 = 1024;

// =============================================================================
// CACHE-ALIGNED ATOMIC WRAPPER
// =============================================================================

/// Cache-line aligned AtomicU64 to prevent false sharing
#[repr(C, align(64))]
struct CacheLineU64 {
    value: AtomicU64,
    _pad: [u8; 56], // 64 - 8 = 56
}

impl CacheLineU64 {
    const fn new(val: u64) -> Self {
        Self {
            value: AtomicU64::new(val),
            _pad: [0; 56],
        }
    }
}

// =============================================================================
// ULTRA-FAST RING
// =============================================================================

/// Ultra-optimized MPMC ring for minimum latency
pub struct UltraFastRing {
    /// Slots array
    slots: Box<[TimestampedSlot]>,
    
    /// Capacity (power of 2)
    capacity: usize,
    
    /// Index mask (capacity - 1)
    mask: u64,
    
    /// Producer head (claim position) - isolated cache line
    producer_head: CacheLineU64,
    
    /// Producer tail (commit position) - isolated cache line
    producer_tail: CacheLineU64,
    
    /// Consumer head (claim position) - isolated cache line
    consumer_head: CacheLineU64,
    
    /// Consumer tail (commit position) - isolated cache line
    consumer_tail: CacheLineU64,
    
    /// Adaptive coalescing controller
    coalesce: CoalesceController,
    
    /// Credit-based flow control
    credits: CreditController,
    
    /// Per-priority statistics
    lane_stats: [LaneStats; 5],
}

unsafe impl Send for UltraFastRing {}
unsafe impl Sync for UltraFastRing {}

impl UltraFastRing {
    /// Create new ultra-fast ring
    pub fn new(capacity: usize) -> Box<Self> {
        assert!(capacity.is_power_of_two(), "Capacity must be power of 2");
        assert!(capacity >= 16, "Minimum capacity is 16");
        
        // Initialize slots with sequence numbers
        let slots: Vec<TimestampedSlot> = (0..capacity)
            .map(|i| TimestampedSlot::new(i as u64))
            .collect();
        
        Box::new(Self {
            slots: slots.into_boxed_slice(),
            capacity,
            mask: (capacity - 1) as u64,
            producer_head: CacheLineU64::new(0),
            producer_tail: CacheLineU64::new(0),
            consumer_head: CacheLineU64::new(0),
            consumer_tail: CacheLineU64::new(0),
            coalesce: CoalesceController::new(),
            credits: CreditController::new(capacity as u64),
            lane_stats: [
                LaneStats::new(),
                LaneStats::new(),
                LaneStats::new(),
                LaneStats::new(),
                LaneStats::new(),
            ],
        })
    }
    
    /// Create with default capacity
    pub fn with_default_capacity() -> Box<Self> {
        Self::new(DEFAULT_CAPACITY)
    }
    
    // ========================================================================
    // INLINE ACCESSORS
    // ========================================================================
    
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    
    #[inline(always)]
    fn slot_index(&self, seq: u64) -> usize {
        (seq & self.mask) as usize
    }
    
    #[inline(always)]
    fn get_slot(&self, seq: u64) -> &TimestampedSlot {
        unsafe { self.slots.get_unchecked(self.slot_index(seq)) }
    }
    
    #[inline]
    pub fn len(&self) -> usize {
        let head = self.producer_tail.value.load(Ordering::Relaxed);
        let tail = self.consumer_tail.value.load(Ordering::Relaxed);
        head.wrapping_sub(tail) as usize
    }
    
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len() >= self.capacity
    }
    
    // ========================================================================
    // ULTRA-FAST SEND (HOT PATH)
    // ========================================================================
    
    /// Ultra-fast inline send - target: 80-100 cycles
    ///
    /// # Arguments
    /// * `data` - Message data (must be ≤40 bytes)
    /// * `priority` - Message priority class
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(QueueFull)` if ring is full
    /// * `Err(InvalidSize)` if data too large
    #[inline]
    pub fn send_fast(&self, data: &[u8], priority: PriorityClass) -> MemoryResult<()> {
        let start_tsc = rdtsc();
        
        // Check size (compile-time for constant sizes)
        if data.len() > FAST_INLINE_MAX {
            return Err(MemoryError::InvalidSize);
        }
        
        // Check credits (flow control)
        if !self.credits.try_consume(1) {
            GLOBAL_PERF_COUNTERS.credit_stalls.fetch_add(1, Ordering::Relaxed);
            return Err(MemoryError::QueueFull);
        }
        
        // Record arrival for coalescing adaptation
        self.coalesce.record_arrival(start_tsc);
        
        // Claim slot with minimal CAS retry
        let seq = self.claim_produce_slot()?;
        
        // Get slot and prefetch next
        let slot = self.get_slot(seq);
        prefetch_write(self.get_slot(seq.wrapping_add(1)) as *const _ as *mut TimestampedSlot);
        
        // Wait for slot to be ready (should be immediate in normal case)
        self.wait_slot_ready_for_write(slot, seq);
        
        // Write data (inlined, no function call)
        unsafe {
            slot.write(data, priority);
        }
        
        // Commit
        self.commit_produce(seq);
        
        // Update stats
        self.lane_stats[priority as usize].record_send(data.len());
        GLOBAL_PERF_COUNTERS.inline_sends.fetch_add(1, Ordering::Relaxed);
        GLOBAL_PERF_COUNTERS.total_send_cycles.fetch_add(
            rdtsc().wrapping_sub(start_tsc),
            Ordering::Relaxed
        );
        
        Ok(())
    }
    
    /// Claim a slot for producing
    #[inline]
    fn claim_produce_slot(&self) -> MemoryResult<u64> {
        let mut head = self.producer_head.value.load(Ordering::Relaxed);
        
        loop {
            // Check if full (use cached consumer tail)
            let tail = self.consumer_tail.value.load(Ordering::Relaxed);
            if head.wrapping_sub(tail) >= self.capacity as u64 {
                return Err(MemoryError::QueueFull);
            }
            
            // Try CAS
            match self.producer_head.value.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(head),
                Err(new) => {
                    head = new;
                    GLOBAL_PERF_COUNTERS.cas_retries.fetch_add(1, Ordering::Relaxed);
                    hint::spin_loop();
                }
            }
        }
    }
    
    /// Wait for slot to be ready for writing
    #[inline]
    fn wait_slot_ready_for_write(&self, slot: &TimestampedSlot, seq: u64) {
        let expected_seq = seq;
        let mut spins = 0u32;
        
        while slot.sequence.load(Ordering::Acquire) != expected_seq {
            hint::spin_loop();
            spins += 1;
            
            if spins > SPIN_LIMIT {
                GLOBAL_PERF_COUNTERS.spin_iterations.fetch_add(spins as u64, Ordering::Relaxed);
                
                if spins > BLOCK_THRESHOLD {
                    // Should never happen in well-behaved system
                    crate::scheduler::yield_now();
                }
            }
        }
    }
    
    /// Commit produced slot
    #[inline]
    fn commit_produce(&self, seq: u64) {
        // Update slot sequence to indicate data is ready
        let slot = self.get_slot(seq);
        slot.sequence.store(seq.wrapping_add(1), Ordering::Release);
        
        // Wait for previous producers to commit (for ordering)
        while self.producer_tail.value.load(Ordering::Acquire) != seq {
            hint::spin_loop();
        }
        
        // Commit
        self.producer_tail.value.store(seq.wrapping_add(1), Ordering::Release);
    }
    
    // ========================================================================
    // ULTRA-FAST RECV (HOT PATH)
    // ========================================================================
    
    /// Ultra-fast receive - target: 80-100 cycles
    #[inline]
    pub fn recv_fast(&self, buffer: &mut [u8]) -> MemoryResult<(usize, PriorityClass, u64)> {
        let start_tsc = rdtsc();
        
        // Claim slot
        let seq = self.claim_consume_slot()?;
        
        // Get slot and prefetch next
        let slot = self.get_slot(seq);
        prefetch_read(self.get_slot(seq.wrapping_add(1)));
        
        // Wait for data
        self.wait_slot_ready_for_read(slot, seq);
        
        // Read data
        let (size, latency) = unsafe { slot.read(buffer) };
        let priority = slot.priority();
        
        // Commit
        self.commit_consume(seq);
        
        // Grant credit back to sender
        self.credits.grant(1);
        
        // Update stats
        self.lane_stats[priority as usize].record_recv(latency);
        GLOBAL_PERF_COUNTERS.total_recv_cycles.fetch_add(
            rdtsc().wrapping_sub(start_tsc),
            Ordering::Relaxed
        );
        
        Ok((size, priority, latency))
    }
    
    /// Claim a slot for consuming
    #[inline]
    fn claim_consume_slot(&self) -> MemoryResult<u64> {
        let mut head = self.consumer_head.value.load(Ordering::Relaxed);
        
        loop {
            // Check if empty
            let tail = self.producer_tail.value.load(Ordering::Acquire);
            if head >= tail {
                return Err(MemoryError::NotFound);
            }
            
            // Try CAS
            match self.consumer_head.value.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(head),
                Err(new) => {
                    head = new;
                    hint::spin_loop();
                }
            }
        }
    }
    
    /// Wait for slot to have data
    #[inline]
    fn wait_slot_ready_for_read(&self, slot: &TimestampedSlot, seq: u64) {
        let expected_seq = seq.wrapping_add(1);
        
        while slot.sequence.load(Ordering::Acquire) != expected_seq {
            hint::spin_loop();
        }
    }
    
    /// Commit consumed slot
    #[inline]
    fn commit_consume(&self, seq: u64) {
        // Mark slot as free for reuse
        let slot = self.get_slot(seq);
        slot.sequence.store(seq.wrapping_add(self.capacity as u64), Ordering::Release);
        
        // Wait for previous consumers
        while self.consumer_tail.value.load(Ordering::Acquire) != seq {
            hint::spin_loop();
        }
        
        // Commit
        self.consumer_tail.value.store(seq.wrapping_add(1), Ordering::Release);
    }
    
    // ========================================================================
    // BLOCKING OPERATIONS
    // ========================================================================
    
    /// Blocking send (with adaptive spinning)
    pub fn send_blocking(&self, data: &[u8], priority: PriorityClass) -> MemoryResult<()> {
        // Try fast path first
        match self.send_fast(data, priority) {
            Ok(()) => return Ok(()),
            Err(MemoryError::QueueFull) => {}
            Err(e) => return Err(e),
        }
        
        // Adaptive spin phase
        let max_batch = self.coalesce.max_batch_size();
        for _ in 0..(SPIN_LIMIT * max_batch) {
            hint::spin_loop();
            if let Ok(()) = self.send_fast(data, priority) {
                return Ok(());
            }
        }
        
        // Block phase
        GLOBAL_PERF_COUNTERS.blocked_waits.fetch_add(1, Ordering::Relaxed);
        loop {
            crate::scheduler::yield_now();
            if let Ok(()) = self.send_fast(data, priority) {
                return Ok(());
            }
        }
    }
    
    /// Blocking receive
    pub fn recv_blocking(&self, buffer: &mut [u8]) -> MemoryResult<(usize, PriorityClass, u64)> {
        // Try fast path first
        match self.recv_fast(buffer) {
            Ok(result) => return Ok(result),
            Err(MemoryError::NotFound) => {}
            Err(e) => return Err(e),
        }
        
        // Spin phase
        for _ in 0..SPIN_LIMIT {
            hint::spin_loop();
            if let Ok(result) = self.recv_fast(buffer) {
                return Ok(result);
            }
        }
        
        // Block phase
        GLOBAL_PERF_COUNTERS.blocked_waits.fetch_add(1, Ordering::Relaxed);
        loop {
            crate::scheduler::yield_now();
            if let Ok(result) = self.recv_fast(buffer) {
                return Ok(result);
            }
        }
    }
    
    // ========================================================================
    // BATCH OPERATIONS
    // ========================================================================
    
    /// Send batch of messages (amortizes overhead)
    /// Returns number of messages successfully sent
    #[inline]
    pub fn send_batch(&self, messages: &[(&[u8], PriorityClass)]) -> usize {
        let mut sent = 0;
        
        for (data, priority) in messages {
            if self.send_fast(data, *priority).is_ok() {
                sent += 1;
            } else {
                break;
            }
        }
        
        if sent > 0 {
            GLOBAL_PERF_COUNTERS.batch_sends.fetch_add(sent as u64, Ordering::Relaxed);
            self.coalesce.flush_batch();
        }
        
        sent
    }
    
    /// Receive batch of messages
    /// Returns vector of (offset, size, priority, latency)
    pub fn recv_batch(&self, max_count: usize, buffer: &mut [u8]) -> Vec<(usize, usize, PriorityClass, u64)> {
        let mut results = Vec::with_capacity(max_count.min(32));
        let mut offset = 0;
        
        for _ in 0..max_count {
            let remaining = &mut buffer[offset..];
            if remaining.len() < FAST_INLINE_MAX {
                break;
            }
            
            match self.recv_fast(remaining) {
                Ok((size, priority, latency)) => {
                    results.push((offset, size, priority, latency));
                    offset += (size + 7) & !7; // Align to 8 bytes
                }
                Err(_) => break,
            }
        }
        
        results
    }
    
    // ========================================================================
    // STATISTICS
    // ========================================================================
    
    /// Get ring statistics
    pub fn stats(&self) -> UltraFastRingStats {
        UltraFastRingStats {
            capacity: self.capacity,
            length: self.len(),
            producer_seq: self.producer_tail.value.load(Ordering::Relaxed),
            consumer_seq: self.consumer_tail.value.load(Ordering::Relaxed),
            coalesce_mode: self.coalesce.mode(),
            available_credits: self.credits.available(),
            lane_stats: [
                self.lane_stats[0].sent.load(Ordering::Relaxed),
                self.lane_stats[1].sent.load(Ordering::Relaxed),
                self.lane_stats[2].sent.load(Ordering::Relaxed),
                self.lane_stats[3].sent.load(Ordering::Relaxed),
                self.lane_stats[4].sent.load(Ordering::Relaxed),
            ],
        }
    }
}

/// Statistics for UltraFastRing
#[derive(Debug, Clone)]
pub struct UltraFastRingStats {
    pub capacity: usize,
    pub length: usize,
    pub producer_seq: u64,
    pub consumer_seq: u64,
    pub coalesce_mode: super::advanced::CoalesceMode,
    pub available_credits: u64,
    pub lane_stats: [u64; 5], // Messages per priority lane
}
