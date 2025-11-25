//! Slot - 64-byte cache-aligned slots for fusion rings
//!
//! Each slot is exactly 64 bytes (one cache line) to avoid false sharing

use core::sync::atomic::{AtomicU64, Ordering};

/// Message slot metadata flags
const SLOT_EMPTY: u64 = 0;
const SLOT_WRITING: u64 = 1;
const SLOT_READY: u64 = 2;
const SLOT_READING: u64 = 3;

/// Fusion ring slot (64 bytes total)
#[repr(C, align(64))]
pub struct Slot {
    /// Atomic status: empty/writing/ready/reading
    pub status: AtomicU64,
    
    /// Message size (bytes)
    pub size: u32,
    
    /// Message flags
    pub flags: u32,
    
    /// Inline data (56 bytes for fast path)
    pub data: [u8; 56],
}

// static_assertions::const_assert_eq!(core::mem::size_of::<Slot>(), 64);

impl Slot {
    pub const fn new() -> Self {
        Self {
            status: AtomicU64::new(SLOT_EMPTY),
            size: 0,
            flags: 0,
            data: [0; 56],
        }
    }
    
    /// Check if slot is empty
    pub fn is_empty(&self) -> bool {
        self.status.load(Ordering::Acquire) == SLOT_EMPTY
    }
    
    /// Check if slot is ready for reading
    pub fn is_ready(&self) -> bool {
        self.status.load(Ordering::Acquire) == SLOT_READY
    }
    
    /// Begin writing (returns true if successful)
    pub fn begin_write(&self) -> bool {
        self.status
            .compare_exchange(SLOT_EMPTY, SLOT_WRITING, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
    
    /// Finish writing (mark slot as ready)
    pub fn finish_write(&self) {
        self.status.store(SLOT_READY, Ordering::Release);
    }
    
    /// Begin reading (returns true if successful)
    pub fn begin_read(&self) -> bool {
        self.status
            .compare_exchange(SLOT_READY, SLOT_READING, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
    
    /// Finish reading (mark slot as empty)
    pub fn finish_read(&self) {
        self.status.store(SLOT_EMPTY, Ordering::Release);
    }
    
    /// Get data pointer (unsafe)
    pub fn data_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }
    
    /// Get mutable data pointer (unsafe)
    pub fn data_mut_ptr(&self) -> *mut u8 {
        self.data.as_ptr() as *mut u8
    }
}
