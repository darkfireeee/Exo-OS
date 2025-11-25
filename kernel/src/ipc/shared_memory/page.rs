//! Shared Memory Page Management
//!
//! Individual page management for shared memory

use crate::memory::{MemoryResult, MemoryError};
use crate::memory::address::{PhysicalAddress, VirtualAddress};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Page size (4KB)
pub const PAGE_SIZE: usize = 4096;

/// Shared page descriptor
pub struct SharedPage {
    /// Physical address
    phys_addr: PhysicalAddress,
    
    /// Reference count
    ref_count: AtomicUsize,
    
    /// Flags
    flags: PageFlags,
}

/// Page flags
#[derive(Debug, Clone, Copy)]
pub struct PageFlags {
    pub writable: bool,
    pub executable: bool,
    pub user_accessible: bool,
    pub write_through: bool,
    pub cache_disabled: bool,
}

impl Default for PageFlags {
    fn default() -> Self {
        Self {
            writable: true,
            executable: false,
            user_accessible: true,
            write_through: false,
            cache_disabled: false,
        }
    }
}

impl SharedPage {
    /// Create new shared page
    pub fn new(phys_addr: PhysicalAddress, flags: PageFlags) -> Self {
        Self {
            phys_addr,
            ref_count: AtomicUsize::new(1),
            flags,
        }
    }
    
    /// Get physical address
    pub fn phys_addr(&self) -> PhysicalAddress {
        self.phys_addr
    }
    
    /// Get reference count
    pub fn ref_count(&self) -> usize {
        self.ref_count.load(Ordering::Acquire)
    }
    
    /// Increment reference count
    pub fn inc_ref(&self) -> usize {
        self.ref_count.fetch_add(1, Ordering::AcqRel) + 1
    }
    
    /// Decrement reference count
    pub fn dec_ref(&self) -> usize {
        let old = self.ref_count.fetch_sub(1, Ordering::AcqRel);
        old.saturating_sub(1)
    }
    
    /// Check if page is shared (ref_count > 1)
    pub fn is_shared(&self) -> bool {
        self.ref_count() > 1
    }
    
    /// Get flags
    pub fn flags(&self) -> PageFlags {
        self.flags
    }
    
    /// Set writable flag
    pub fn set_writable(&mut self, writable: bool) {
        self.flags.writable = writable;
    }
}

/// Allocate shared page
pub fn alloc_shared_page(flags: PageFlags) -> MemoryResult<SharedPage> {
    // TODO: Use proper physical frame allocator when API complete
    // For now, return dummy physical address
    let phys_addr = PhysicalAddress::new(0x1000_0000);
    Ok(SharedPage::new(phys_addr, flags))
}

/// Free shared page if ref_count reaches 0
pub fn free_shared_page(page: &SharedPage) -> MemoryResult<()> {
    if page.ref_count() == 0 {
        // TODO: Free physical frame when deallocator API complete
        log::debug!("Would free page at phys {:?}", page.phys_addr);
        Ok(())
    } else {
        Err(MemoryError::PermissionDenied)
    }
}

/// Clone shared page (increment ref count)
pub fn clone_shared_page(page: &SharedPage) -> SharedPage {
    page.inc_ref();
    SharedPage::new(page.phys_addr, page.flags)
}
