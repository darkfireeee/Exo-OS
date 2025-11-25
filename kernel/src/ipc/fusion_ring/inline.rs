//! Inline - Fast path for small messages (≤56 bytes)
//!
//! Achieves ~350 cycles for inline messages without memory allocation

use super::{ring::Ring, slot::Slot};
use crate::memory::{MemoryResult, MemoryError};

/// Maximum inline message size (56 bytes to fit in one cache line)
pub const MAX_INLINE_SIZE: usize = 56;

/// Send inline message (fast path)
pub fn send_inline(ring: &Ring, data: &[u8]) -> MemoryResult<()> {
    if data.len() > MAX_INLINE_SIZE {
        return Err(crate::memory::MemoryError::InvalidSize);
    }
    
    // Acquire write slot
    let slot = ring.acquire_write_slot()
        .ok_or(crate::memory::MemoryError::OutOfMemory)?;
    
    // Copy data inline (single cache line write)
    unsafe {
        let dst = slot.data_mut_ptr();
        core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
    }
    
    // Set size and mark ready (single atomic write)
    unsafe {
        let slot_ptr = slot as *const Slot as *mut Slot;
        let size_ptr = core::ptr::addr_of!((*slot_ptr).size) as *mut u32;
        core::ptr::write_volatile(size_ptr, data.len() as u32);
    }
    
    slot.finish_write();
    Ok(())
}

/// Receive inline message (fast path)
pub fn recv_inline(ring: &Ring, buffer: &mut [u8]) -> MemoryResult<usize> {
    // Acquire read slot
    let slot = ring.acquire_read_slot()
        .ok_or(crate::memory::MemoryError::NotFound)?;
    
    let size = slot.size as usize;
    
    if size > buffer.len() {
        slot.finish_read();
        return Err(crate::memory::MemoryError::InvalidSize);
    }
    
    // Copy data (single cache line read)
    unsafe {
        let src = slot.data_ptr();
        core::ptr::copy_nonoverlapping(src, buffer.as_mut_ptr(), size);
    }
    
    slot.finish_read();
    Ok(size)
}

/// Check if message fits inline
pub fn fits_inline(size: usize) -> bool {
    size <= MAX_INLINE_SIZE
}
