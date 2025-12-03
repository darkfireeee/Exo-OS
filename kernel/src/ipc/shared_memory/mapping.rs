//! Virtual Memory Mapping for Shared Memory
//!
//! Maps shared physical pages into process virtual address spaces

use crate::memory::{MemoryResult, MemoryError};
use crate::memory::address::{PhysicalAddress, VirtualAddress};
use crate::memory::virtual_mem;
use crate::memory::PageProtection;
use super::page::{SharedPage, PageFlags, PAGE_SIZE};
use alloc::vec::Vec;
use spin::Mutex;
use alloc::collections::BTreeMap;

/// Mapping flags
#[derive(Debug, Clone, Copy)]
pub struct MappingFlags {
    pub read: bool,
    pub write: bool,
    pub exec: bool,
    pub user: bool,
}

impl MappingFlags {
    pub const READ_ONLY: Self = Self { read: true, write: false, exec: false, user: true };
    pub const READ_WRITE: Self = Self { read: true, write: true, exec: false, user: true };
    pub const READ_EXEC: Self = Self { read: true, write: false, exec: true, user: true };
    
    /// Convert to PageProtection for virtual_mem API
    pub fn to_protection(&self) -> PageProtection {
        let mut prot = PageProtection::new();
        if self.read {
            prot = prot.read();
        }
        if self.write {
            prot = prot.write();
        }
        if self.exec {
            prot = prot.execute();
        }
        prot
    }
}

/// Shared memory mapping
pub struct SharedMapping {
    /// Virtual base address
    virt_addr: VirtualAddress,
    
    /// Physical pages
    pages: Vec<SharedPage>,
    
    /// Size in bytes
    size: usize,
    
    /// Mapping flags
    flags: MappingFlags,
    
    /// Whether mapping is currently active
    is_mapped: bool,
}

impl SharedMapping {
    /// Create new mapping
    pub fn new(virt_addr: VirtualAddress, pages: Vec<SharedPage>, flags: MappingFlags) -> Self {
        let size = pages.len() * PAGE_SIZE;
        Self {
            virt_addr,
            pages,
            size,
            flags,
            is_mapped: false,
        }
    }
    
    /// Get virtual address
    pub fn virt_addr(&self) -> VirtualAddress {
        self.virt_addr
    }
    
    /// Get size
    pub fn size(&self) -> usize {
        self.size
    }
    
    /// Get page count
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    
    /// Get physical address for offset
    pub fn phys_addr_at(&self, offset: usize) -> MemoryResult<PhysicalAddress> {
        let page_idx = offset / PAGE_SIZE;
        if page_idx >= self.pages.len() {
            return Err(MemoryError::InvalidAddress);
        }
        
        let page_offset = offset % PAGE_SIZE;
        let page_phys = self.pages[page_idx].phys_addr();
        Ok(PhysicalAddress::new(page_phys.value() + page_offset))
    }
    
    /// Map pages into virtual address space using real page tables
    pub fn map(&mut self) -> MemoryResult<()> {
        if self.is_mapped {
            return Ok(()); // Already mapped
        }
        
        let protection = self.flags.to_protection();
        
        for (i, page) in self.pages.iter().enumerate() {
            let virt = VirtualAddress::new(self.virt_addr.value() + i * PAGE_SIZE);
            let phys = page.phys_addr();
            
            // Use virtual_mem API to map the page
            virtual_mem::map_page(virt, phys, protection)?;
            
            log::trace!("SharedMapping: mapped {:?} -> {:?}", virt, phys);
        }
        
        self.is_mapped = true;
        log::debug!("SharedMapping: mapped {} pages at {:?}", self.pages.len(), self.virt_addr);
        Ok(())
    }
    
    /// Unmap pages from virtual address space
    pub fn unmap(&mut self) -> MemoryResult<()> {
        if !self.is_mapped {
            return Ok(()); // Not mapped
        }
        
        for i in 0..self.pages.len() {
            let virt = VirtualAddress::new(self.virt_addr.value() + i * PAGE_SIZE);
            
            // Use virtual_mem API to unmap the page
            virtual_mem::unmap_page(virt)?;
            
            log::trace!("SharedMapping: unmapped {:?}", virt);
        }
        
        self.is_mapped = false;
        log::debug!("SharedMapping: unmapped {} pages at {:?}", self.pages.len(), self.virt_addr);
        Ok(())
    }
    
    /// Change protection (mprotect-like)
    pub fn protect(&mut self, new_flags: MappingFlags) -> MemoryResult<()> {
        if !self.is_mapped {
            // Just update flags if not mapped
            self.flags = new_flags;
            return Ok(());
        }
        
        let protection = new_flags.to_protection();
        
        // Need to remap pages with new protection
        for i in 0..self.pages.len() {
            let virt = VirtualAddress::new(self.virt_addr.value() + i * PAGE_SIZE);
            let phys = self.pages[i].phys_addr();
            
            // Unmap then remap with new protection
            virtual_mem::unmap_page(virt)?;
            virtual_mem::map_page(virt, phys, protection)?;
        }
        
        self.flags = new_flags;
        Ok(())
    }
    
    /// Check if mapping is active
    pub fn is_mapped(&self) -> bool {
        self.is_mapped
    }
}

impl Drop for SharedMapping {
    fn drop(&mut self) {
        // Unmap pages first
        let _ = self.unmap();
        
        // Decrement ref count for all pages
        for page in &self.pages {
            page.dec_ref();
        }
    }
}

/// Global mapping tracker for address space management
static MAPPING_ALLOCATOR: Mutex<MappingAllocator> = Mutex::new(MappingAllocator::new());

/// Virtual address allocator for shared mappings
struct MappingAllocator {
    /// Next available virtual address
    next_addr: usize,
    /// End of allocatable region
    end_addr: usize,
    /// Active mappings: virt_addr -> size
    active_mappings: Option<BTreeMap<usize, usize>>,
}

impl MappingAllocator {
    const fn new() -> Self {
        Self {
            // Use 0x5000_0000 - 0x6000_0000 for shared mappings (256MB)
            next_addr: 0x5000_0000,
            end_addr: 0x6000_0000,
            active_mappings: None,
        }
    }
    
    fn ensure_init(&mut self) {
        if self.active_mappings.is_none() {
            self.active_mappings = Some(BTreeMap::new());
        }
    }
    
    /// Allocate virtual address range
    fn allocate(&mut self, size: usize) -> Option<VirtualAddress> {
        self.ensure_init();
        
        // Align size to page boundary
        let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        
        // Simple bump allocator (could be improved with free list)
        if self.next_addr + aligned_size > self.end_addr {
            return None;
        }
        
        let addr = self.next_addr;
        self.next_addr += aligned_size;
        
        // Track allocation
        if let Some(ref mut mappings) = self.active_mappings {
            mappings.insert(addr, aligned_size);
        }
        
        Some(VirtualAddress::new(addr))
    }
    
    /// Release virtual address range (for reuse)
    fn release(&mut self, addr: usize) {
        self.ensure_init();
        
        if let Some(ref mut mappings) = self.active_mappings {
            mappings.remove(&addr);
            // Note: Simple allocator doesn't coalesce or reuse freed ranges
            // A production allocator would maintain a free list
        }
    }
}

/// Map shared memory into process address space
pub fn map_shared(phys_addr: PhysicalAddress, size: usize, virt_addr: VirtualAddress, flags: MappingFlags) -> MemoryResult<SharedMapping> {
    // Calculate page count
    let page_count = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    
    // Create page descriptors
    let mut pages = Vec::with_capacity(page_count);
    for i in 0..page_count {
        let page_phys = PhysicalAddress::new(phys_addr.value() + i * PAGE_SIZE);
        let page_flags = PageFlags {
            writable: flags.write,
            executable: flags.exec,
            user_accessible: flags.user,
            ..Default::default()
        };
        pages.push(SharedPage::new(page_phys, page_flags));
    }
    
    let mut mapping = SharedMapping::new(virt_addr, pages, flags);
    mapping.map()?;
    
    Ok(mapping)
}

/// Map shared memory with automatic virtual address allocation
pub fn map_shared_auto(phys_addr: PhysicalAddress, size: usize, flags: MappingFlags) -> MemoryResult<SharedMapping> {
    let mut allocator = MAPPING_ALLOCATOR.lock();
    let virt_addr = allocator.allocate(size)
        .ok_or(MemoryError::OutOfMemory)?;
    drop(allocator); // Release lock before mapping
    
    map_shared(phys_addr, size, virt_addr, flags)
}

/// Unmap shared memory
pub fn unmap_shared(mut mapping: SharedMapping) -> MemoryResult<()> {
    // Release virtual address range
    let mut allocator = MAPPING_ALLOCATOR.lock();
    allocator.release(mapping.virt_addr().value());
    drop(allocator);
    
    // Unmap pages (done in Drop, but do it explicitly)
    mapping.unmap()?;
    
    // Drop will handle page ref counting
    drop(mapping);
    Ok(())
}

/// Get mapping statistics
pub fn mapping_stats() -> MappingStats {
    let allocator = MAPPING_ALLOCATOR.lock();
    let bytes_used = allocator.next_addr - 0x5000_0000;
    let active_count = allocator.active_mappings.as_ref().map(|m| m.len()).unwrap_or(0);
    
    MappingStats {
        bytes_allocated: bytes_used,
        active_mappings: active_count,
        available_bytes: 0x6000_0000 - allocator.next_addr,
    }
}

/// Statistics for mapping allocator
#[derive(Debug, Clone, Copy)]
pub struct MappingStats {
    /// Total bytes allocated
    pub bytes_allocated: usize,
    /// Number of active mappings
    pub active_mappings: usize,
    /// Available bytes remaining
    pub available_bytes: usize,
}
