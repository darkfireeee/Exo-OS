//! Virtual Memory Mapping for Shared Memory
//!
//! Maps shared physical pages into process virtual address spaces

use crate::memory::{MemoryResult, MemoryError};
use crate::memory::address::{PhysicalAddress, VirtualAddress};
use super::page::{SharedPage, PageFlags};
use alloc::vec::Vec;

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
}

impl SharedMapping {
    /// Create new mapping
    pub fn new(virt_addr: VirtualAddress, pages: Vec<SharedPage>, flags: MappingFlags) -> Self {
        let size = pages.len() * 4096;
        Self {
            virt_addr,
            pages,
            size,
            flags,
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
        let page_idx = offset / 4096;
        if page_idx >= self.pages.len() {
            return Err(MemoryError::InvalidAddress);
        }
        
        let page_offset = offset % 4096;
        let page_phys = self.pages[page_idx].phys_addr();
        Ok(PhysicalAddress::new(page_phys.value() + page_offset))
    }
    
    /// Map pages into virtual address space
    pub fn map(&self) -> MemoryResult<()> {
        // TODO: Integration complète avec page_table quand disponible
        // Pour l'instant, stub qui réussit
        log::debug!("Mapping {} pages at {:?}", self.pages.len(), self.virt_addr);
        Ok(())
    }
    
    /// Unmap pages from virtual address space
    pub fn unmap(&self) -> MemoryResult<()> {
        // TODO: Integration complète avec page_table quand disponible
        log::debug!("Unmapping {} pages at {:?}", self.pages.len(), self.virt_addr);
        Ok(())
    }
    
    /// Change protection (mprotect-like)
    pub fn protect(&mut self, flags: MappingFlags) -> MemoryResult<()> {
        // TODO: Integration complète avec page_table quand disponible
        self.flags = flags;
        Ok(())
    }
}

impl Drop for SharedMapping {
    fn drop(&mut self) {
        let _ = self.unmap();
        
        // Decrement ref count for all pages
        for page in &self.pages {
            page.dec_ref();
        }
    }
}

/// Map shared memory into process address space
pub fn map_shared(phys_addr: PhysicalAddress, size: usize, virt_addr: VirtualAddress, flags: MappingFlags) -> MemoryResult<SharedMapping> {
    // Calculate page count
    let page_count = (size + 4095) / 4096;
    
    // Create page descriptors
    let mut pages = Vec::new();
    for i in 0..page_count {
        let page_phys = PhysicalAddress::new(phys_addr.value() + i * 4096);
        let page_flags = PageFlags {
            writable: flags.write,
            executable: flags.exec,
            user_accessible: flags.user,
            ..Default::default()
        };
        pages.push(SharedPage::new(page_phys, page_flags));
    }
    
    let mapping = SharedMapping::new(virt_addr, pages, flags);
    mapping.map()?;
    
    Ok(mapping)
}

/// Unmap shared memory
pub fn unmap_shared(mapping: SharedMapping) -> MemoryResult<()> {
    drop(mapping);
    Ok(())
}
