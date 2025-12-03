//! SlotV2 - Next-generation cache-optimized message slots
//!
//! Each slot is exactly 64 bytes (one cache line) to prevent false sharing.
//! The slot format is optimized for the common inline path.
//!
//! ## Slot Layout (64 bytes total):
//! ```text
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! | State  | Flags  |   Size (16b)    |      Sequence (32b)               |  Header (8B)
//! +--------+--------+--------+--------+--------+--------+--------+--------+
//! |                                                                       |
//! |                        Inline Data (56 bytes)                         |  Payload
//! |                                                                       |
//! +-----------------------------------------------------------------------+
//! ```
//!
//! For zero-copy messages, the payload contains a physical address instead.

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use core::ptr;

/// Slot size (one cache line)
pub const SLOT_SIZE: usize = 64;

/// Maximum inline payload size
pub const MAX_INLINE_PAYLOAD: usize = 56;

/// Slot state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SlotState {
    /// Slot is free and can be claimed for writing
    Empty = 0,
    /// Slot is being written by a producer
    Writing = 1,
    /// Slot contains valid data ready for consumption
    Ready = 2,
    /// Slot is being read by a consumer
    Reading = 3,
}

impl From<u8> for SlotState {
    fn from(v: u8) -> Self {
        match v {
            0 => SlotState::Empty,
            1 => SlotState::Writing,
            2 => SlotState::Ready,
            3 => SlotState::Reading,
            _ => SlotState::Empty,
        }
    }
}

/// Slot flags
pub mod flags {
    /// Message is inline (payload in slot)
    pub const INLINE: u8 = 0;
    /// Message uses zero-copy (payload is physical address)
    pub const ZEROCOPY: u8 = 1 << 0;
    /// Message is part of a batch
    pub const BATCH: u8 = 1 << 1;
    /// Message has priority
    pub const PRIORITY: u8 = 1 << 2;
    /// Message requires acknowledgment
    pub const NEED_ACK: u8 = 1 << 3;
    /// Message is a response
    pub const RESPONSE: u8 = 1 << 4;
}

/// Slot header (8 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SlotHeader {
    /// Slot state (Empty/Writing/Ready/Reading)
    pub state: u8,
    /// Message flags
    pub flags: u8,
    /// Payload size in bytes
    pub size: u16,
    /// Sequence number for ordering (lower 32 bits)
    pub sequence: u32,
}

impl SlotHeader {
    pub const fn empty() -> Self {
        Self {
            state: SlotState::Empty as u8,
            flags: 0,
            size: 0,
            sequence: 0,
        }
    }
    
    #[inline(always)]
    pub fn is_inline(&self) -> bool {
        self.flags & flags::ZEROCOPY == 0
    }
    
    #[inline(always)]
    pub fn is_zerocopy(&self) -> bool {
        self.flags & flags::ZEROCOPY != 0
    }
}

/// Message slot (64 bytes, cache-line aligned)
#[repr(C, align(64))]
pub struct SlotV2 {
    /// Atomic header for lock-free operations
    /// Layout: [state:8][flags:8][size:16][seq:32]
    header: AtomicU64,
    
    /// Payload data (56 bytes for inline, or physical address for zerocopy)
    payload: [u8; MAX_INLINE_PAYLOAD],
}

// Verify size at compile time
const _: () = assert!(core::mem::size_of::<SlotV2>() == SLOT_SIZE);

impl SlotV2 {
    /// Create new empty slot
    pub const fn new() -> Self {
        Self {
            header: AtomicU64::new(0),
            payload: [0u8; MAX_INLINE_PAYLOAD],
        }
    }
    
    /// Create slot with specific sequence
    pub const fn with_sequence(seq: u32) -> Self {
        let header = (seq as u64) << 32;
        Self {
            header: AtomicU64::new(header),
            payload: [0u8; MAX_INLINE_PAYLOAD],
        }
    }
    
    /// Load header atomically
    #[inline(always)]
    fn load_header(&self, order: Ordering) -> SlotHeader {
        let raw = self.header.load(order);
        SlotHeader {
            state: (raw & 0xFF) as u8,
            flags: ((raw >> 8) & 0xFF) as u8,
            size: ((raw >> 16) & 0xFFFF) as u16,
            sequence: ((raw >> 32) & 0xFFFFFFFF) as u32,
        }
    }
    
    /// Store header atomically
    #[inline(always)]
    fn store_header(&self, header: SlotHeader, order: Ordering) {
        let raw = (header.state as u64)
            | ((header.flags as u64) << 8)
            | ((header.size as u64) << 16)
            | ((header.sequence as u64) << 32);
        self.header.store(raw, order);
    }
    
    /// Compare-and-swap header atomically
    #[inline(always)]
    fn cas_header(
        &self,
        expected: SlotHeader,
        new: SlotHeader,
        success: Ordering,
        failure: Ordering,
    ) -> Result<SlotHeader, SlotHeader> {
        let expected_raw = (expected.state as u64)
            | ((expected.flags as u64) << 8)
            | ((expected.size as u64) << 16)
            | ((expected.sequence as u64) << 32);
        let new_raw = (new.state as u64)
            | ((new.flags as u64) << 8)
            | ((new.size as u64) << 16)
            | ((new.sequence as u64) << 32);
        
        match self.header.compare_exchange(expected_raw, new_raw, success, failure) {
            Ok(v) => Ok(SlotHeader {
                state: (v & 0xFF) as u8,
                flags: ((v >> 8) & 0xFF) as u8,
                size: ((v >> 16) & 0xFFFF) as u16,
                sequence: ((v >> 32) & 0xFFFFFFFF) as u32,
            }),
            Err(v) => Err(SlotHeader {
                state: (v & 0xFF) as u8,
                flags: ((v >> 8) & 0xFF) as u8,
                size: ((v >> 16) & 0xFFFF) as u16,
                sequence: ((v >> 32) & 0xFFFFFFFF) as u32,
            }),
        }
    }
    
    /// Get current state
    #[inline(always)]
    pub fn state(&self) -> SlotState {
        let raw = self.header.load(Ordering::Acquire);
        SlotState::from((raw & 0xFF) as u8)
    }
    
    /// Get sequence number
    #[inline(always)]
    pub fn sequence(&self) -> u32 {
        let raw = self.header.load(Ordering::Acquire);
        ((raw >> 32) & 0xFFFFFFFF) as u32
    }
    
    /// Try to begin writing (Empty -> Writing)
    /// Returns true if successful
    #[inline]
    pub fn try_begin_write(&self, expected_seq: u32) -> bool {
        let expected = SlotHeader {
            state: SlotState::Empty as u8,
            flags: 0,
            size: 0,
            sequence: expected_seq,
        };
        let new = SlotHeader {
            state: SlotState::Writing as u8,
            flags: 0,
            size: 0,
            sequence: expected_seq,
        };
        
        self.cas_header(expected, new, Ordering::Acquire, Ordering::Relaxed).is_ok()
    }
    
    /// Write inline data to slot
    /// SAFETY: Caller must have successfully called try_begin_write
    #[inline]
    pub unsafe fn write_inline(&self, data: &[u8], flags: u8) {
        debug_assert!(data.len() <= MAX_INLINE_PAYLOAD);
        
        // Copy data directly (single cache line write)
        let dst = self.payload.as_ptr() as *mut u8;
        ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        
        // Update header with size and mark ready
        let header = self.load_header(Ordering::Relaxed);
        let new_header = SlotHeader {
            state: SlotState::Ready as u8,
            flags: flags | flags::INLINE,
            size: data.len() as u16,
            sequence: header.sequence,
        };
        self.store_header(new_header, Ordering::Release);
    }
    
    /// Write zero-copy reference to slot
    /// SAFETY: Caller must have successfully called try_begin_write
    #[inline]
    pub unsafe fn write_zerocopy(&self, phys_addr: u64, size: usize, flags: u8) {
        // Store physical address in payload
        let dst = self.payload.as_ptr() as *mut u64;
        ptr::write_volatile(dst, phys_addr);
        
        // Store size in next 8 bytes (for >64KB messages)
        let size_ptr = dst.add(1);
        ptr::write_volatile(size_ptr, size as u64);
        
        // Update header
        let header = self.load_header(Ordering::Relaxed);
        let new_header = SlotHeader {
            state: SlotState::Ready as u8,
            flags: flags | flags::ZEROCOPY,
            size: if size <= u16::MAX as usize { size as u16 } else { 0 },
            sequence: header.sequence,
        };
        self.store_header(new_header, Ordering::Release);
    }
    
    /// Try to begin reading (Ready -> Reading)
    #[inline]
    pub fn try_begin_read(&self, expected_seq: u32) -> Option<(usize, u8)> {
        let header = self.load_header(Ordering::Acquire);
        
        if header.state != SlotState::Ready as u8 || header.sequence != expected_seq {
            return None;
        }
        
        let new_header = SlotHeader {
            state: SlotState::Reading as u8,
            ..header
        };
        
        match self.cas_header(header, new_header, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => Some((header.size as usize, header.flags)),
            Err(_) => None,
        }
    }
    
    /// Read inline data from slot
    /// SAFETY: Caller must have successfully called try_begin_read
    #[inline]
    pub unsafe fn read_inline(&self, buffer: &mut [u8], size: usize) {
        debug_assert!(size <= MAX_INLINE_PAYLOAD);
        debug_assert!(size <= buffer.len());
        
        let src = self.payload.as_ptr();
        ptr::copy_nonoverlapping(src, buffer.as_mut_ptr(), size);
    }
    
    /// Read zero-copy reference from slot
    /// Returns (physical_address, size)
    #[inline]
    pub unsafe fn read_zerocopy(&self) -> (u64, usize) {
        let src = self.payload.as_ptr() as *const u64;
        let phys_addr = ptr::read_volatile(src);
        let size = ptr::read_volatile(src.add(1)) as usize;
        (phys_addr, size)
    }
    
    /// Finish reading and release slot (Reading -> Empty)
    /// Advances sequence for next use
    #[inline]
    pub fn finish_read(&self, capacity: u32) {
        let header = self.load_header(Ordering::Relaxed);
        let new_seq = header.sequence.wrapping_add(capacity);
        
        let new_header = SlotHeader {
            state: SlotState::Empty as u8,
            flags: 0,
            size: 0,
            sequence: new_seq,
        };
        self.store_header(new_header, Ordering::Release);
    }
    
    /// Get payload pointer for direct access
    #[inline(always)]
    pub fn payload_ptr(&self) -> *const u8 {
        self.payload.as_ptr()
    }
    
    /// Get mutable payload pointer
    #[inline(always)]
    pub fn payload_mut_ptr(&self) -> *mut u8 {
        self.payload.as_ptr() as *mut u8
    }
}

impl Default for SlotV2 {
    fn default() -> Self {
        Self::new()
    }
}
