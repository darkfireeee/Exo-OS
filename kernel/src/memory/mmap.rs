//! Memory mapping (mmap) implementation
//! 
//! Provides POSIX-compatible mmap/munmap functionality with real page table mapping

use super::{VirtualAddress, PhysicalAddress, MemoryError, MemoryResult, PageProtection};
use super::virtual_mem::page_table::{PageTableFlags, PageTableWalker};
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
    pub const STACK: u32 = 0x20000;      // MAP_STACK
    pub const GROWSDOWN: u32 = 0x100;    // MAP_GROWSDOWN
    
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
    
    pub const fn is_populate(&self) -> bool {
        self.0 & Self::POPULATE != 0
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
    /// Is this mapping COW (copy-on-write)?
    pub is_cow: bool,
}

impl MmapEntry {
    pub fn virt_end(&self) -> VirtualAddress {
        VirtualAddress::new(self.virt_start.value() + self.size)
    }
    
    pub fn contains(&self, addr: VirtualAddress) -> bool {
        addr >= self.virt_start && addr < self.virt_end()
    }
    
    pub fn page_count(&self) -> usize {
        (self.size + 4095) / 4096
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
    
    /// Create a new memory mapping (with real page table mapping)
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
                // Use address as hint, find nearby if not available
                if self.is_range_available(addr.value(), aligned_size) {
                    addr
                } else {
                    self.find_available_range(aligned_size)?
                }
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
        
        // Convert protection to page table flags
        let pt_flags = protection_to_flags(protection);
        
        // Map pages in the page table
        if flags.is_anonymous() && !frames.is_empty() {
            // Get current CR3 and create page table walker
            let cr3 = unsafe { 
                let cr3: u64;
                core::arch::asm!("mov {}, cr3", out(reg) cr3);
                PhysicalAddress::new(cr3 as usize)
            };
            
            let mut walker = PageTableWalker::new(cr3);
            
            for (i, &frame) in frames.iter().enumerate() {
                let page_addr = VirtualAddress::new(virt_start.value() + i * page_size);
                
                if let Err(e) = walker.map(page_addr, frame, pt_flags) {
                    // Rollback: unmap already mapped pages
                    for j in 0..i {
                        let unmap_addr = VirtualAddress::new(virt_start.value() + j * page_size);
                        let _ = walker.unmap(unmap_addr);
                    }
                    // Free allocated frames
                    for frame in &frames {
                        let f = super::physical::Frame::containing_address(*frame);
                        let _ = super::physical::deallocate_frame(f);
                    }
                    return Err(e);
                }
            }
            
            // Zero the pages if anonymous
            unsafe {
                core::ptr::write_bytes(virt_start.value() as *mut u8, 0, aligned_size);
            }
            
            // Flush TLB for mapped range
            for i in 0..(aligned_size / page_size) {
                let addr = virt_start.value() + i * page_size;
                unsafe {
                    core::arch::asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags));
                }
            }
        }
        
        // Create mapping entry
        let entry = MmapEntry {
            virt_start,
            size: aligned_size,
            protection,
            flags,
            frames,
            fd,
            offset,
            is_cow: false,
        };
        
        // Store mapping
        self.mappings.insert(virt_start.value(), entry);
        
        log::debug!("mmap: mapped {:#x}-{:#x} ({} pages)", 
            virt_start.value(), 
            virt_start.value() + aligned_size,
            aligned_size / page_size
        );
        
        Ok(virt_start)
    }
    
    /// Unmap memory region (with real page table unmapping)
    pub fn munmap(&mut self, addr: VirtualAddress, size: usize) -> MemoryResult<()> {
        let page_size = 4096;
        let aligned_size = (size + page_size - 1) & !(page_size - 1);
        
        // Find overlapping mappings
        let mut to_remove = Vec::new();
        
        for (&start, entry) in self.mappings.iter() {
            if entry.contains(addr) || 
               (addr.value() < start + entry.size && addr.value() + aligned_size > start) {
                to_remove.push(start);
            }
        }
        
        if to_remove.is_empty() {
            return Err(MemoryError::NotMapped);
        }
        
        // Get current CR3
        let cr3 = unsafe {
            let cr3: u64;
            core::arch::asm!("mov {}, cr3", out(reg) cr3);
            PhysicalAddress::new(cr3 as usize)
        };
        let mut walker = PageTableWalker::new(cr3);
        
        // Remove mappings
        for start in to_remove {
            if let Some(entry) = self.mappings.remove(&start) {
                // Unmap pages from page table
                let num_pages = entry.page_count();
                for i in 0..num_pages {
                    let page_addr = VirtualAddress::new(entry.virt_start.value() + i * page_size);
                    let _ = walker.unmap(page_addr);
                    
                    // Flush TLB entry
                    unsafe {
                        core::arch::asm!(
                            "invlpg [{}]", 
                            in(reg) page_addr.value(), 
                            options(nostack, preserves_flags)
                        );
                    }
                }
                
                // Free physical frames
                for frame in entry.frames {
                    let f = super::physical::Frame::containing_address(frame);
                    let _ = super::physical::deallocate_frame(f);
                }
                
                log::debug!("munmap: unmapped {:#x}-{:#x}", 
                    entry.virt_start.value(), 
                    entry.virt_start.value() + entry.size
                );
            }
        }
        
        Ok(())
    }
    
    /// Change protection of memory region (with real page table update)
    pub fn mprotect(
        &mut self,
        addr: VirtualAddress,
        size: usize,
        protection: PageProtection,
    ) -> MemoryResult<()> {
        let page_size = 4096;
        
        // Get current CR3
        let cr3 = unsafe {
            let cr3: u64;
            core::arch::asm!("mov {}, cr3", out(reg) cr3);
            PhysicalAddress::new(cr3 as usize)
        };
        let mut walker = PageTableWalker::new(cr3);
        
        // Convert protection to page table flags
        let pt_flags = protection_to_flags(protection);
        
        // Find mapping and update protection
        for entry in self.mappings.values_mut() {
            if entry.contains(addr) {
                entry.protection = protection;
                
                // Update page table flags for all pages in mapping
                let num_pages = entry.page_count();
                for i in 0..num_pages {
                    let page_addr = VirtualAddress::new(entry.virt_start.value() + i * page_size);
                    let _ = walker.protect(page_addr, pt_flags);
                    
                    // Flush TLB entry
                    unsafe {
                        core::arch::asm!(
                            "invlpg [{}]", 
                            in(reg) page_addr.value(), 
                            options(nostack, preserves_flags)
                        );
                    }
                }
                
                log::debug!("mprotect: updated protection for {:#x}", addr.value());
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

/// Convert PageProtection to PageTableFlags
fn protection_to_flags(prot: PageProtection) -> PageTableFlags {
    let mut flags = PageTableFlags::new().present().user();
    
    if prot.is_writable() {
        flags = flags.writable();
    }
    
    if !prot.is_executable() {
        flags = flags.no_execute();
    }
    
    flags
}

/// Check if an address is mapped
pub fn is_mapped(addr: VirtualAddress) -> bool {
    let manager = MMAP_MANAGER.lock();
    if let Some(ref mgr) = *manager {
        for entry in mgr.mappings.values() {
            if entry.contains(addr) {
                return true;
            }
        }
    }
    false
}

/// Get mapping info at address
pub fn get_mapping_info(addr: VirtualAddress) -> Option<(VirtualAddress, usize, PageProtection)> {
    let manager = MMAP_MANAGER.lock();
    if let Some(ref mgr) = *manager {
        for entry in mgr.mappings.values() {
            if entry.contains(addr) {
                return Some((entry.virt_start, entry.size, entry.protection));
            }
        }
    }
    None
}

/// Memory-mapped region advise (madvise)
pub fn madvise(addr: VirtualAddress, size: usize, advice: i32) -> MemoryResult<()> {
    const MADV_DONTNEED: i32 = 4;
    const MADV_WILLNEED: i32 = 3;
    
    match advice {
        MADV_DONTNEED => {
            // Mark pages as not needed - can be discarded
            log::debug!("madvise: DONTNEED for {:#x} size={}", addr.value(), size);
            Ok(())
        }
        MADV_WILLNEED => {
            // Hint that pages will be needed soon - prefetch
            log::debug!("madvise: WILLNEED for {:#x} size={}", addr.value(), size);
            Ok(())
        }
        _ => {
            // Unknown advice, just ignore
            Ok(())
        }
    }
}

/// Memory lock (mlock) - prevent pages from being swapped
pub fn mlock(addr: VirtualAddress, size: usize) -> MemoryResult<()> {
    let mut manager = MMAP_MANAGER.lock();
    let manager = manager.as_mut().ok_or(MemoryError::InternalError("mmap not initialized"))?;
    
    for entry in manager.mappings.values_mut() {
        if entry.contains(addr) {
            // Mark mapping as locked
            entry.flags = MmapFlags::new(entry.flags.0 | MmapFlags::LOCKED);
            log::debug!("mlock: locked {:#x}", addr.value());
            return Ok(());
        }
    }
    
    Err(MemoryError::NotMapped)
}

/// Memory unlock (munlock)
pub fn munlock(addr: VirtualAddress, size: usize) -> MemoryResult<()> {
    let mut manager = MMAP_MANAGER.lock();
    let manager = manager.as_mut().ok_or(MemoryError::InternalError("mmap not initialized"))?;
    
    for entry in manager.mappings.values_mut() {
        if entry.contains(addr) {
            // Remove locked flag
            entry.flags = MmapFlags::new(entry.flags.0 & !MmapFlags::LOCKED);
            log::debug!("munlock: unlocked {:#x}", addr.value());
            return Ok(());
        }
    }
    
    Err(MemoryError::NotMapped)
}

/// Sync memory mapping to file (msync)
pub fn msync(addr: VirtualAddress, size: usize, flags: i32) -> MemoryResult<()> {
    let manager = MMAP_MANAGER.lock();
    let manager = manager.as_ref().ok_or(MemoryError::InternalError("mmap not initialized"))?;
    
    for entry in manager.mappings.values() {
        if entry.contains(addr) {
            if let Some(fd) = entry.fd {
                // TODO: Sync to file via VFS
                log::debug!("msync: would sync fd {} at {:#x}", fd, addr.value());
            }
            return Ok(());
        }
    }
    
    Err(MemoryError::NotMapped)
}
