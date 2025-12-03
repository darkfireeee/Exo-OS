//! Shared Memory Page Management
//!
//! Individual page management for shared memory

use crate::memory::{MemoryResult, MemoryError};
use crate::memory::address::{PhysicalAddress, VirtualAddress};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Page size (4KB)
pub const PAGE_SIZE: usize = 4096;

/// Shared page descriptor
#[derive(Debug)]
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

use spin::Mutex;
use alloc::collections::BTreeMap;

/// Global shared page allocator
static SHARED_PAGE_ALLOCATOR: Mutex<SharedPageAllocator> = Mutex::new(SharedPageAllocator::new());

/// Shared page allocator with reference counting
struct SharedPageAllocator {
    /// Next allocation address (bump allocator for simplicity)
    next_addr: usize,
    /// End of allocatable region
    end_addr: usize,
    /// Active allocations: phys_addr -> ref_count
    allocations: Option<BTreeMap<usize, usize>>,
}

impl SharedPageAllocator {
    const fn new() -> Self {
        Self {
            // Start at 32MB, reserve 0x2000000 - 0x4000000 (32MB) for shared pages
            next_addr: 0x2000_0000,
            end_addr: 0x4000_0000,
            allocations: None,
        }
    }
    
    fn ensure_init(&mut self) {
        if self.allocations.is_none() {
            self.allocations = Some(BTreeMap::new());
        }
    }
    
    fn allocate(&mut self) -> Option<PhysicalAddress> {
        self.ensure_init();
        
        if self.next_addr + PAGE_SIZE > self.end_addr {
            return None;
        }
        
        let addr = self.next_addr;
        self.next_addr += PAGE_SIZE;
        
        // Track allocation
        if let Some(ref mut allocs) = self.allocations {
            allocs.insert(addr, 1);
        }
        
        log::trace!("SharedPageAllocator: allocated page at {:#x}", addr);
        Some(PhysicalAddress::new(addr))
    }
    
    fn retain(&mut self, addr: usize) -> bool {
        self.ensure_init();
        
        if let Some(ref mut allocs) = self.allocations {
            if let Some(ref_count) = allocs.get_mut(&addr) {
                *ref_count += 1;
                return true;
            }
        }
        false
    }
    
    fn release(&mut self, addr: usize) -> Option<usize> {
        self.ensure_init();
        
        if let Some(ref mut allocs) = self.allocations {
            if let Some(ref_count) = allocs.get_mut(&addr) {
                *ref_count -= 1;
                let remaining = *ref_count;
                if remaining == 0 {
                    allocs.remove(&addr);
                    // Note: Physical memory is NOT reclaimed in this simple allocator
                    // A production allocator would add addr back to free list
                    log::trace!("SharedPageAllocator: released page at {:#x}", addr);
                }
                return Some(remaining);
            }
        }
        None
    }
}

/// Allocate shared page using the real allocator
pub fn alloc_shared_page(flags: PageFlags) -> MemoryResult<SharedPage> {
    let mut allocator = SHARED_PAGE_ALLOCATOR.lock();
    
    let phys_addr = allocator.allocate()
        .ok_or(MemoryError::OutOfMemory)?;
    
    // Zero the page for security
    // Safety: We just allocated this address, it's valid and ours
    unsafe {
        // In kernel with identity mapping, phys == virt for low addresses
        let ptr = phys_addr.value() as *mut u8;
        core::ptr::write_bytes(ptr, 0, PAGE_SIZE);
    }
    
    Ok(SharedPage::new(phys_addr, flags))
}

/// Allocate multiple contiguous shared pages
pub fn alloc_shared_pages(count: usize, flags: PageFlags) -> MemoryResult<alloc::vec::Vec<SharedPage>> {
    let mut pages = alloc::vec::Vec::with_capacity(count);
    
    for _ in 0..count {
        pages.push(alloc_shared_page(flags)?);
    }
    
    Ok(pages)
}

/// Free shared page if ref_count reaches 0
pub fn free_shared_page(page: &SharedPage) -> MemoryResult<()> {
    let remaining = page.dec_ref();
    
    if remaining == 0 {
        // Release from allocator
        let mut allocator = SHARED_PAGE_ALLOCATOR.lock();
        allocator.release(page.phys_addr.value());
        Ok(())
    } else {
        // Page still has references, nothing to do
        Ok(())
    }
}

/// Clone shared page (increment ref count)
pub fn clone_shared_page(page: &SharedPage) -> SharedPage {
    page.inc_ref();
    
    // Also track in allocator
    let mut allocator = SHARED_PAGE_ALLOCATOR.lock();
    allocator.retain(page.phys_addr.value());
    
    SharedPage::new(page.phys_addr, page.flags)
}

/// Get statistics about shared page allocator
pub fn shared_page_stats() -> SharedPageStats {
    let allocator = SHARED_PAGE_ALLOCATOR.lock();
    let allocated_pages = (allocator.next_addr - 0x2000_0000) / PAGE_SIZE;
    let active_pages = allocator.allocations.as_ref().map(|a| a.len()).unwrap_or(0);
    
    SharedPageStats {
        total_allocated: allocated_pages,
        currently_active: active_pages,
        bytes_used: allocated_pages * PAGE_SIZE,
    }
}

/// Statistics for shared page allocator
#[derive(Debug, Clone, Copy)]
pub struct SharedPageStats {
    /// Total pages ever allocated
    pub total_allocated: usize,
    /// Currently active (non-freed) pages
    pub currently_active: usize,
    /// Total bytes used
    pub bytes_used: usize,
}
