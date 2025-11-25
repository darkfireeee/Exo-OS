//! Physical frame structure and operations

use crate::memory::PhysicalAddress;

/// Frame size (4KB)
pub const FRAME_SIZE: usize = 4096;

/// Physical memory frame
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Frame {
    /// Starting physical address
    pub start: PhysicalAddress,
}

impl Frame {
    /// Create frame from physical address
    pub const fn new(addr: PhysicalAddress) -> Self {
        Self { start: addr }
    }
    
    /// Get frame containing given address
    pub const fn containing_address(addr: PhysicalAddress) -> Self {
        Self {
            start: PhysicalAddress::new(addr.value() & !(FRAME_SIZE - 1)),
        }
    }
    
    /// Get starting address
    pub const fn address(&self) -> PhysicalAddress {
        self.start
    }
    
    /// Get ending address (exclusive)
    pub const fn end_address(&self) -> PhysicalAddress {
        PhysicalAddress::new(self.start.value() + FRAME_SIZE)
    }
    
    /// Get frame number
    pub const fn number(&self) -> usize {
        self.start.value() / FRAME_SIZE
    }
    
    /// Get next frame
    pub const fn next(&self) -> Self {
        Self::new(PhysicalAddress::new(self.start.value() + FRAME_SIZE))
    }
    
    /// Get range of frames
    pub fn range(start: Frame, end: Frame) -> FrameRange {
        FrameRange { start, end }
    }
}

/// Range of physical frames
#[derive(Debug, Clone)]
pub struct FrameRange {
    pub start: Frame,
    pub end: Frame,
}

impl Iterator for FrameRange {
    type Item = Frame;
    
    fn next(&mut self) -> Option<Self::Item> {
        if self.start < self.end {
            let frame = self.start;
            self.start = self.start.next();
            Some(frame)
        } else {
            None
        }
    }
}
