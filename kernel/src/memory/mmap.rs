//! Memory mapping (mmap) implementation
//! 
//! Provides POSIX-compatible mmap/munmap functionality

use super::{VirtualAddress, PhysicalAddress, MemoryError, MemoryResult, PageProtection};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;

/// Memory mapping flags (POSIX compatible)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmapFlags(pub u32);

impl MmapFlags {
    pub const SHARED: u32 = 0x01;        // MAP_SHARED
    pub const PRIVATE: u32 = 0x02;       // MAP_PRIVATE
    pub const FIXED: u32 = 0x10;         // MAP_FIXED
    pub const ANONYMOUS: u32 = 0x20;     // MAP_ANONYMOUS
    pub const POPULATE: u32 = 0x8000;    // MAP_POPULATE
    pub const LOCKED: u32 = 0x2000;      // MAP_LOCKED
    
    pub const fn new(flags: u32) -> Self {
        Self(flags)
    }
    
    pub const fn is_shared(&self) -> bool {
        self.0 & Self::SHARED != 0
    }
    
    pub const fn is_private(&self) -> bool {
        self.0 & Self::PRIVATE != 0
    }
    
    pub const fn is_fixed(&self) -> bool {
        self.0 & Self::FIXED != 0
    }
    
    pub const fn is_anonymous(&self) -> bool {
        self.0 & Self::ANONYMOUS != 0
    }
}

/// Memory mapping entry
#[derive(Debug)]
pub struct MmapEntry {
    /// Virtual address start
    pub virt_start: VirtualAddress,
    /// Size in bytes
    pub size: usize,
    /// Protection flags
    pub protection: PageProtection,
    /// Mapping flags
    pub flags: MmapFlags,
    /// Physical frames (for anonymous mappings)
    pub frames: Vec<PhysicalAddress>,
    /// File descriptor (for file-backed mappings)
    pub fd: Option<i32>,
    /// Offset in file
    pub offset: usize,
}

impl MmapEntry {
    pub fn virt_end(&self) -> VirtualAddress {
        VirtualAddress::new(self.virt_start.value() + self.size)
    }
    
    pub fn contains(&self, addr: VirtualAddress) -> bool {
        addr >= self.virt_start && addr < self.virt_end()
    }
}

/// Memory map manager
pub struct MmapManager {
    /// Active mappings (keyed by virtual address)
    mappings: BTreeMap<usize, MmapEntry>,
    /// Next available address for anonymous mappings
    next_addr: usize,
}

impl MmapManager {
    pub fn new() -> Self {
        Self {
            mappings: BTreeMap::new(),
            // Start anonymous mappings at 2GB
            next_addr: 0x8000_0000,
        }
    }
    
    /// Create a new memory mapping
    pub fn mmap(
        &mut self,
        addr: Option<VirtualAddress>,
        size: usize,
        protection: PageProtection,
        flags: MmapFlags,
        fd: Option<i32>,
        offset: usize,
    ) -> MemoryResult<VirtualAddress> {
        // Round size up to page boundary
        let page_size = 4096;
        let aligned_size = (size + page_size - 1) & !(page_size - 1);
        
        // Determine virtual address
        let virt_start = if let Some(addr) = addr {
            if flags.is_fixed() {
                // Use fixed address (must be available)
                if self.is_range_available(addr.value(), aligned_size) {
                    addr
                } else {
                    return Err(MemoryError::AlreadyMapped);
                }
            } else {
                // Use address as hint
                self.find_available_range(aligned_size)?
            }
        } else {
            // Find any available range
            self.find_available_range(aligned_size)?
        };
        
        // Allocate physical frames for anonymous mappings
        let frames = if flags.is_anonymous() {
            self.allocate_frames(aligned_size / page_size)?
        } else {
            Vec::new()
        };
        
        // Create mapping entry
        let entry = MmapEntry {
            virt_start,
            size: aligned_size,
            protection,
            flags,
            frames,
            fd,
            offset,
        };
        
        // TODO: Actually map pages in page table
        
        // Store mapping
        self.mappings.insert(virt_start.value(), entry);
        
        Ok(virt_start)
    }
    
    /// Unmap memory region
    pub fn munmap(&mut self, addr: VirtualAddress, size: usize) -> MemoryResult<()> {
        // Find overlapping mappings
        let mut to_remove = Vec::new();
        
        for (&start, entry) in self.mappings.iter() {
            if entry.contains(addr) {
                to_remove.push(start);
            }
        }
        
        // Remove mappings
        for start in to_remove {
            if let Some(entry) = self.mappings.remove(&start) {
                // TODO: Unmap pages from page table
                // TODO: Free physical frames
                drop(entry);
            }
        }
        
        Ok(())
    }
    
    /// Change protection of memory region
    pub fn mprotect(
        &mut self,
        addr: VirtualAddress,
        size: usize,
        protection: PageProtection,
    ) -> MemoryResult<()> {
        // Find mapping
        for entry in self.mappings.values_mut() {
            if entry.contains(addr) {
                entry.protection = protection;
                // TODO: Update page table permissions
                return Ok(());
            }
        }
        
        Err(MemoryError::NotMapped)
    }
    
    /// Find available address range
    fn find_available_range(&mut self, size: usize) -> MemoryResult<VirtualAddress> {
        let addr = self.next_addr;
        self.next_addr += size;
        Ok(VirtualAddress::new(addr))
    }
    
    /// Check if address range is available
    fn is_range_available(&self, start: usize, size: usize) -> bool {
        let end = start + size;
        
        for entry in self.mappings.values() {
            let entry_start = entry.virt_start.value();
            let entry_end = entry_start + entry.size;
            
            // Check for overlap
            if !(end <= entry_start || start >= entry_end) {
                return false;
            }
        }
        
        true
    }
    
    /// Allocate physical frames
    fn allocate_frames(&self, count: usize) -> MemoryResult<Vec<PhysicalAddress>> {
        let mut frames = Vec::new();
        
        for _ in 0..count {
            match super::physical::buddy_allocator::alloc_frame() {
                Ok(frame) => frames.push(frame),
                Err(e) => {
                    // Free already allocated frames
                    for frame in frames {
                        let _ = super::physical::buddy_allocator::free_frame(frame);
                    }
                    return Err(e);
                }
            }
        }
        
        Ok(frames)
    }
}

/// Global mmap manager (per-process in real implementation)
static MMAP_MANAGER: Mutex<Option<MmapManager>> = Mutex::new(None);

/// Initialize mmap subsystem
pub fn init() {
    let mut manager = MMAP_MANAGER.lock();
    *manager = Some(MmapManager::new());
}

/// POSIX mmap wrapper
pub fn mmap(
    addr: Option<VirtualAddress>,
    size: usize,
    protection: PageProtection,
    flags: MmapFlags,
    fd: Option<i32>,
    offset: usize,
) -> MemoryResult<VirtualAddress> {
    let mut manager = MMAP_MANAGER.lock();
    let manager = manager.as_mut().ok_or(MemoryError::InternalError("mmap not initialized"))?;
    
    manager.mmap(addr, size, protection, flags, fd, offset)
}

/// POSIX munmap wrapper
pub fn munmap(addr: VirtualAddress, size: usize) -> MemoryResult<()> {
    let mut manager = MMAP_MANAGER.lock();
    let manager = manager.as_mut().ok_or(MemoryError::InternalError("mmap not initialized"))?;
    
    manager.munmap(addr, size)
}

/// POSIX mprotect wrapper
pub fn mprotect(addr: VirtualAddress, size: usize, protection: PageProtection) -> MemoryResult<()> {
    let mut manager = MMAP_MANAGER.lock();
    let manager = manager.as_mut().ok_or(MemoryError::InternalError("mmap not initialized"))?;
    
    manager.mprotect(addr, size, protection)
}
