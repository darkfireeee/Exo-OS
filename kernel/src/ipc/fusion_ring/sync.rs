//! Sync - Synchronization primitives for fusion rings
//!
//! Provides wait/notify mechanisms for blocking operations

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use super::ring::Ring;
use crate::memory::{MemoryResult, MemoryError};
use crate::scheduler::{yield_now, block_current};

/// Ring synchronizer
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
    
    /// Wait for data to be available
    pub fn wait_readable(&self, ring: &Ring) {
        // Fast path: check if data available
        if !ring.is_empty() {
            return;
        }
        
        // Spin briefly (100 iterations ~= 300 cycles)
        for _ in 0..100 {
            core::hint::spin_loop();
            if !ring.is_empty() {
                return;
            }
        }
        
        // Still empty, block current thread
        self.reader_wake.store(false, Ordering::Release);
        
        // Store current thread ID for unblocking
        // Note: In real impl, would get from scheduler
        // self.blocked_reader.store(current_tid, Ordering::Release);
        
        // Double-check before blocking
        if ring.is_empty() {
            // Block current thread (integrated with scheduler V2)
            block_current();
        }
    }
    
    /// Wait for space to be available
    pub fn wait_writable(&self, ring: &Ring) {
        // Fast path: check if space available
        if !ring.is_full() {
            return;
        }
        
        // Spin briefly (100 iterations ~= 300 cycles)
        for _ in 0..100 {
            core::hint::spin_loop();
            if !ring.is_full() {
                return;
            }
        }
        
        // Still full, block current thread
        self.writer_wake.store(false, Ordering::Release);
        
        // Store current thread ID for unblocking
        // self.blocked_writer.store(current_tid, Ordering::Release);
        
        // Double-check before blocking
        if ring.is_full() {
            // Block current thread (integrated with scheduler V2)
            block_current();
        }
    }
    
    /// Notify waiting readers
    pub fn notify_readers(&self) {
        if !self.reader_wake.swap(true, Ordering::AcqRel) {
            // Wake blocked reader thread if any
            let reader_tid = self.blocked_reader.swap(0, Ordering::AcqRel);
            if reader_tid != 0 {
                // Note: In real impl, would call scheduler::unblock(reader_tid)
                // For now, the thread will wake on next timer interrupt
            }
        }
    }
    
    /// Notify waiting writers
    pub fn notify_writers(&self) {
        if !self.writer_wake.swap(true, Ordering::AcqRel) {
            // Wake blocked writer thread if any
            let writer_tid = self.blocked_writer.swap(0, Ordering::AcqRel);
            if writer_tid != 0 {
                // Note: In real impl, would call scheduler::unblock(writer_tid)
                // For now, the thread will wake on next timer interrupt
            }
        }
    }
}

/// Blocking send (waits for space)
pub fn send_blocking(ring: &Ring, sync: &RingSync, data: &[u8]) -> MemoryResult<()> {
    sync.wait_writable(ring);
    
    let result = super::inline::send_inline(ring, data);
    
    // Notify readers
    sync.notify_readers();
    
    result
}

/// Blocking receive (waits for data)
pub fn recv_blocking(ring: &Ring, sync: &RingSync, buffer: &mut [u8]) -> MemoryResult<usize> {
    sync.wait_readable(ring);
    
    let result = super::inline::recv_inline(ring, buffer);
    
    // Notify writers
    sync.notify_writers();
    
    result
}
