//! Sync - Synchronization primitives for fusion rings
//!
//! Provides wait/notify mechanisms for blocking operations
//! Fully integrated with Exo-OS scheduler for real thread blocking

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use super::ring::Ring;
use crate::memory::{MemoryResult, MemoryError};
use crate::scheduler::{yield_now, block_current, unblock};

/// Maximum spin iterations before blocking (tuned for performance)
const SPIN_ITERATIONS: u32 = 100;

/// Ring synchronizer with real scheduler integration
pub struct RingSync {
    /// Wake flag for waiting readers
    reader_wake: AtomicBool,
    
    /// Wake flag for waiting writers
    writer_wake: AtomicBool,
    
    /// Blocked reader thread ID (0 = none)
    blocked_reader: AtomicU64,
    
    /// Blocked writer thread ID (0 = none)
    blocked_writer: AtomicU64,
}

impl RingSync {
    pub const fn new() -> Self {
        Self {
            reader_wake: AtomicBool::new(false),
            writer_wake: AtomicBool::new(false),
            blocked_reader: AtomicU64::new(0),
            blocked_writer: AtomicU64::new(0),
        }
    }
    
    /// Get current thread ID from scheduler
    #[inline]
    fn current_tid() -> u64 {
        crate::scheduler::SCHEDULER.current_thread_id().unwrap_or(0)
    }
    
    /// Wait for data to be available (reader side)
    pub fn wait_readable(&self, ring: &Ring) {
        // Fast path: check if data available
        if !ring.is_empty() {
            return;
        }
        
        // Adaptive spin: brief spinning before blocking
        for _ in 0..SPIN_ITERATIONS {
            core::hint::spin_loop();
            if !ring.is_empty() {
                return;
            }
        }
        
        // Slow path: block on scheduler
        loop {
            // Reset wake flag
            self.reader_wake.store(false, Ordering::Release);
            
            // Store current thread ID for wake-up
            let tid = Self::current_tid();
            if tid != 0 {
                self.blocked_reader.store(tid, Ordering::Release);
            }
            
            // Double-check condition before blocking
            if ring.is_empty() {
                // Block current thread - will be woken by notify_readers
                block_current();
            }
            
            // Clear blocked state
            self.blocked_reader.store(0, Ordering::Release);
            
            // Check if we should wake
            if !ring.is_empty() || self.reader_wake.load(Ordering::Acquire) {
                return;
            }
        }
    }
    
    /// Wait for space to be available (writer side)
    pub fn wait_writable(&self, ring: &Ring) {
        // Fast path: check if space available
        if !ring.is_full() {
            return;
        }
        
        // Adaptive spin: brief spinning before blocking
        for _ in 0..SPIN_ITERATIONS {
            core::hint::spin_loop();
            if !ring.is_full() {
                return;
            }
        }
        
        // Slow path: block on scheduler
        loop {
            // Reset wake flag
            self.writer_wake.store(false, Ordering::Release);
            
            // Store current thread ID for wake-up
            let tid = Self::current_tid();
            if tid != 0 {
                self.blocked_writer.store(tid, Ordering::Release);
            }
            
            // Double-check condition before blocking
            if ring.is_full() {
                // Block current thread - will be woken by notify_writers
                block_current();
            }
            
            // Clear blocked state
            self.blocked_writer.store(0, Ordering::Release);
            
            // Check if we should wake
            if !ring.is_full() || self.writer_wake.load(Ordering::Acquire) {
                return;
            }
        }
    }
    
    /// Notify waiting readers that data is available
    pub fn notify_readers(&self) {
        // Set wake flag
        self.reader_wake.store(true, Ordering::Release);
        
        // Wake blocked reader thread if any
        let reader_tid = self.blocked_reader.swap(0, Ordering::AcqRel);
        if reader_tid != 0 {
            // Call scheduler to unblock the thread (ThreadId is u64)
            unblock(reader_tid);
        }
    }
    
    /// Notify waiting writers that space is available
    pub fn notify_writers(&self) {
        // Set wake flag
        self.writer_wake.store(true, Ordering::Release);
        
        // Wake blocked writer thread if any
        let writer_tid = self.blocked_writer.swap(0, Ordering::AcqRel);
        if writer_tid != 0 {
            // Call scheduler to unblock the thread (ThreadId is u64)
            unblock(writer_tid);
        }
    }
    
    /// Notify all (both readers and writers)
    pub fn notify_all(&self) {
        self.notify_readers();
        self.notify_writers();
    }
}

/// Blocking send (waits for space if ring is full)
pub fn send_blocking(ring: &Ring, sync: &RingSync, data: &[u8]) -> MemoryResult<()> {
    // Wait for space
    sync.wait_writable(ring);
    
    // Send data
    let result = super::inline::send_inline(ring, data);
    
    // Notify readers that data is available
    if result.is_ok() {
        sync.notify_readers();
    }
    
    result
}

/// Blocking receive (waits for data if ring is empty)
pub fn recv_blocking(ring: &Ring, sync: &RingSync, buffer: &mut [u8]) -> MemoryResult<usize> {
    // Wait for data
    sync.wait_readable(ring);
    
    // Receive data
    let result = super::inline::recv_inline(ring, buffer);
    
    // Notify writers that space is available
    if result.is_ok() {
        sync.notify_writers();
    }
    
    result
}

/// Try send with timeout (cycles-based)
pub fn send_with_timeout(ring: &Ring, sync: &RingSync, data: &[u8], max_spins: u64) -> MemoryResult<()> {
    let mut spins = 0u64;
    
    while ring.is_full() {
        if spins >= max_spins {
            return Err(MemoryError::Timeout);
        }
        core::hint::spin_loop();
        spins += 1;
    }
    
    let result = super::inline::send_inline(ring, data);
    if result.is_ok() {
        sync.notify_readers();
    }
    result
}

/// Try receive with timeout (cycles-based)
pub fn recv_with_timeout(ring: &Ring, sync: &RingSync, buffer: &mut [u8], max_spins: u64) -> MemoryResult<usize> {
    let mut spins = 0u64;
    
    while ring.is_empty() {
        if spins >= max_spins {
            return Err(MemoryError::Timeout);
        }
        core::hint::spin_loop();
        spins += 1;
    }
    
    let result = super::inline::recv_inline(ring, buffer);
    if result.is_ok() {
        sync.notify_writers();
    }
    result
}
