//! DMA (Direct Memory Access) utilities
//! 
//! Provides memory allocation and management for DMA operations

use super::{PhysicalAddress, VirtualAddress, MemoryError, MemoryResult};
use super::physical::buddy_allocator;
use alloc::vec::Vec;
use spin::Mutex;

/// DMA buffer alignment (typically 4KB for page alignment)
pub const DMA_ALIGNMENT: usize = 4096;

/// DMA memory region
pub struct DmaRegion {
    /// Virtual address
    virt_addr: VirtualAddress,
    /// Physical address (contiguous)
    phys_addr: PhysicalAddress,
    /// Size in bytes
    size: usize,
}

impl DmaRegion {
    /// Allocate a new DMA region
    pub fn new(size: usize) -> MemoryResult<Self> {
        // Round up size to page boundary
        let aligned_size = (size + DMA_ALIGNMENT - 1) & !(DMA_ALIGNMENT - 1);
        let num_frames = aligned_size / 4096;
        
        // Allocate contiguous physical frames
        let phys_addr = buddy_allocator::alloc_contiguous(num_frames)?;
        
        // Map to virtual memory (identity map for simplicity)
        // In production, use proper virtual memory mapping
        let virt_addr = VirtualAddress::new(phys_addr.value() as usize);
        
        Ok(Self {
            virt_addr,
            phys_addr,
            size: aligned_size,
        })
    }
    
    /// Get virtual address
    pub fn virt_addr(&self) -> VirtualAddress {
        self.virt_addr
    }
    
    /// Get physical address (for DMA controller)
    pub fn phys_addr(&self) -> PhysicalAddress {
        self.phys_addr
    }
    
    /// Get size
    pub fn size(&self) -> usize {
        self.size
    }
    
    /// Get as mutable slice
    pub unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
        core::slice::from_raw_parts_mut(self.virt_addr.value() as *mut u8, self.size)
    }
    
    /// Get as slice
    pub unsafe fn as_slice(&self) -> &[u8] {
        core::slice::from_raw_parts(self.virt_addr.value() as *const u8, self.size)
    }
    
    /// Write data to DMA buffer
    pub unsafe fn write(&mut self, data: &[u8]) -> MemoryResult<()> {
        if data.len() > self.size {
            return Err(MemoryError::InvalidSize);
        }
        
        let slice = self.as_mut_slice();
        slice[..data.len()].copy_from_slice(data);
        
        // Flush cache to ensure DMA controller sees the data
        crate::memory::cache::clflush_range(self.virt_addr.value(), data.len());
        
        Ok(())
    }
    
    /// Read data from DMA buffer
    pub unsafe fn read(&self, buffer: &mut [u8]) -> MemoryResult<()> {
        if buffer.len() > self.size {
            return Err(MemoryError::InvalidSize);
        }
        
        // Invalidate cache to ensure we read fresh data from memory
        crate::memory::cache::clflush_range(self.virt_addr.value(), buffer.len());
        
        let slice = self.as_slice();
        buffer.copy_from_slice(&slice[..buffer.len()]);
        
        Ok(())
    }
}

impl Drop for DmaRegion {
    fn drop(&mut self) {
        // Free physical frames
        let num_frames = self.size / 4096;
        for i in 0..num_frames {
            let frame = PhysicalAddress::new(self.phys_addr.value() + i * 4096);
            let _ = buddy_allocator::free_frame(frame);
        }
    }
}

/// DMA pool for efficient allocation of small buffers
pub struct DmaPool {
    buffer_size: usize,
    regions: Mutex<Vec<DmaRegion>>,
}

impl DmaPool {
    /// Create a new DMA pool
    pub fn new(buffer_size: usize) -> Self {
        Self {
            buffer_size,
            regions: Mutex::new(Vec::new()),
        }
    }
    
    /// Allocate a buffer from the pool
    pub fn alloc(&self) -> MemoryResult<DmaRegion> {
        // For now, just allocate a new region
        // TODO: Implement actual pooling with recycling
        DmaRegion::new(self.buffer_size)
    }
    
    /// Return a buffer to the pool
    pub fn free(&self, _region: DmaRegion) {
        // TODO: Implement buffer recycling
        // For now, just drop it
    }
}

/// DMA direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaDirection {
    /// Device to memory (read)
    ToMemory,
    /// Memory to device (write)
    ToDevice,
    /// Bidirectional
    Bidirectional,
}

/// DMA transfer descriptor
pub struct DmaTransfer {
    pub phys_addr: PhysicalAddress,
    pub size: usize,
    pub direction: DmaDirection,
}

impl DmaTransfer {
    pub fn new(phys_addr: PhysicalAddress, size: usize, direction: DmaDirection) -> Self {
        Self {
            phys_addr,
            size,
            direction,
        }
    }
}
