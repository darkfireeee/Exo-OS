//! Physical memory management

pub mod bitmap_allocator;
pub mod numa;

use crate::memory::{PhysicalAddress, MemoryError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub start: PhysicalAddress,
}

impl Frame {
    pub fn new(addr: PhysicalAddress) -> Self {
        Frame { start: addr }
    }
    
    pub const fn containing_address(addr: PhysicalAddress) -> Self {
        Frame { start: PhysicalAddress::new(addr.value() & !0xFFF) }
    }
    
    pub const fn address(&self) -> PhysicalAddress {
        self.start
    }
}

pub fn allocate_frame() -> Result<Frame, crate::memory::MemoryError> {
    // Stub pour allocation
    Err(crate::memory::MemoryError::OutOfMemory)
}

pub fn deallocate_frame(_frame: Frame) -> Result<(), crate::memory::MemoryError> {
    // Stub pour désallocation
    Ok(())
}
