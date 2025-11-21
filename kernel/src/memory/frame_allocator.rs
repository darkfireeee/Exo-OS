pub use x86_64::structures::paging::FrameAllocator;
use x86_64::{
    PhysAddr,
    structures::paging::{PhysFrame, Size4KiB},
};

/// Simple frame allocator for early boot
#[derive(Debug)]
pub struct SimpleFrameAllocator {
    next_frame: PhysAddr,
    end_frame: PhysAddr,
}

// SimpleFrameAllocator is Send because it only contains PhysAddr which is Send
unsafe impl Send for SimpleFrameAllocator {}

impl SimpleFrameAllocator {
    pub fn new() -> Self {
        Self {
            next_frame: PhysAddr::new(0x100000), // Start after 1MB
            end_frame: PhysAddr::new(0x10000000), // 256MB default
        }
    }

    pub fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        if self.next_frame >= self.end_frame {
            return None;
        }

        let frame = PhysFrame::containing_address(self.next_frame);
        self.next_frame += 4096u64; // 4KB frame size
        Some(frame)
    }
    
    /// Add a memory range to the allocator
    pub fn add_range(&mut self, start: PhysAddr, end: PhysAddr) {
        // If this range extends our current range, update it
        if start < self.next_frame {
            self.next_frame = start;
        }
        if end > self.end_frame {
            self.end_frame = end;
        }
    }
}

// Implement FrameAllocator trait for SimpleFrameAllocator
unsafe impl FrameAllocator<Size4KiB> for SimpleFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.allocate_frame()
    }
}
