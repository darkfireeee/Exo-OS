//! Inline - Fast path for small messages (≤56 bytes)
//!
//! Achieves ~150 cycles for inline messages without memory allocation
//! Optimized for the common case of small IPC messages

use super::ring::Ring;
use super::slot::Slot;
use crate::memory::{MemoryResult, MemoryError};

/// Maximum inline message size (56 bytes to fit in one cache line)
pub const MAX_INLINE_SIZE: usize = 56;

/// Send inline message (fast path)
///
/// # Performance: ~150 cycles
/// - Acquire slot: ~20 cycles (CAS)
/// - Copy data: ~50 cycles (memcpy for ≤56 bytes)
/// - Release: ~20 cycles (store release)
pub fn send_inline(ring: &Ring, data: &[u8]) -> MemoryResult<()> {
    if data.len() > MAX_INLINE_SIZE {
        return Err(MemoryError::InvalidSize);
    }
    
    // Acquire write slot from MPMC ring
    let slot = ring.acquire_write_slot()
        .ok_or(MemoryError::QueueFull)?;
    
    // Copy data inline (single cache line write)
    // The RingSlot.data is a Slot which has 56 bytes of inline storage
    unsafe {
        let dst = slot.data.data_mut_ptr();
        core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
    }
    
    // Set size (flags = 0 indicates inline message)
    unsafe {
        let slot_ptr = &slot.data as *const Slot as *mut Slot;
        let size_ptr = core::ptr::addr_of_mut!((*slot_ptr).size);
        core::ptr::write_volatile(size_ptr, data.len() as u32);
        
        let flags_ptr = core::ptr::addr_of_mut!((*slot_ptr).flags);
        core::ptr::write_volatile(flags_ptr, 0); // 0 = inline
    }
    
    // Complete write - advances sequence, makes slot visible to consumers
    ring.complete_write(slot);
    
    Ok(())
}

/// Receive inline message (fast path)
///
/// # Performance: ~150 cycles
pub fn recv_inline(ring: &Ring, buffer: &mut [u8]) -> MemoryResult<usize> {
    // Acquire read slot from MPMC ring
    let slot = ring.acquire_read_slot()
        .ok_or(MemoryError::NotFound)?;
    
    // Check if this is a zerocopy message
    if slot.data.flags != 0 {
        // Not inline - let caller handle zerocopy
        ring.complete_read(slot);
        return Err(MemoryError::InvalidParameter);
    }
    
    let size = slot.data.size as usize;
    
    if size > buffer.len() {
        ring.complete_read(slot);
        return Err(MemoryError::InvalidSize);
    }
    
    // Copy data (single cache line read)
    unsafe {
        let src = slot.data.data_ptr();
        core::ptr::copy_nonoverlapping(src, buffer.as_mut_ptr(), size);
    }
    
    // Complete read - recycles slot for producers
    ring.complete_read(slot);
    
    Ok(size)
}

/// Try receive without blocking
/// 
/// Returns WouldBlock if no message available
pub fn try_recv_inline(ring: &Ring, buffer: &mut [u8]) -> MemoryResult<usize> {
    if ring.is_empty() {
        return Err(MemoryError::WouldBlock);
    }
    recv_inline(ring, buffer)
}

/// Check if message fits inline
#[inline(always)]
pub fn fits_inline(size: usize) -> bool {
    size <= MAX_INLINE_SIZE
}
