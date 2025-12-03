//! User Space Page Tables
//! 
//! Simplified page table management for user processes.
//! Creates and manages page tables for user-space memory regions.

use crate::memory::{PhysicalAddress, VirtualAddress, MemoryError, PAGE_SIZE};
use alloc::vec;
use alloc::vec::Vec;
use core::ptr;

/// Page table entry flags
#[derive(Debug, Clone, Copy)]
pub struct UserPageFlags(u64);

impl UserPageFlags {
    /// No flags
    pub const fn empty() -> Self {
        Self(0)
    }
    
    /// Present bit
    pub const fn present(self) -> Self {
        Self(self.0 | (1 << 0))
    }
    
    /// Writable bit
    pub const fn writable(self) -> Self {
        Self(self.0 | (1 << 1))
    }
    
    /// User-accessible bit (Ring 3 can access)
    pub const fn user(self) -> Self {
        Self(self.0 | (1 << 2))
    }
    
    /// Write-through bit
    pub const fn write_through(self) -> Self {
        Self(self.0 | (1 << 3))
    }
    
    /// Cache disable bit
    pub const fn cache_disable(self) -> Self {
        Self(self.0 | (1 << 4))
    }
    
    /// No-execute bit (requires NX support)
    pub const fn no_execute(self) -> Self {
        Self(self.0 | (1 << 63))
    }
    
    /// Get raw value
    pub const fn bits(self) -> u64 {
        self.0
    }
    
    /// Standard flags for user code (R-X)
    pub const fn user_code() -> Self {
        Self::empty().present().user()
        // Note: No writable, no no_execute = executable
    }
    
    /// Standard flags for user data (RW-)
    pub const fn user_data() -> Self {
        Self::empty().present().user().writable().no_execute()
    }
    
    /// Standard flags for user stack (RW-)
    pub const fn user_stack() -> Self {
        Self::empty().present().user().writable().no_execute()
    }
    
    /// Standard flags for user read-only data (R--)
    pub const fn user_rodata() -> Self {
        Self::empty().present().user().no_execute()
    }
}

/// A page table entry (PTE)
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// Empty entry (not present)
    pub const fn empty() -> Self {
        Self(0)
    }
    
    /// Create entry with physical address and flags
    pub fn new(phys_addr: PhysicalAddress, flags: UserPageFlags) -> Self {
        // Physical address must be page-aligned
        let addr = phys_addr.value() as u64 & 0x000F_FFFF_FFFF_F000;
        Self(addr | flags.bits())
    }
    
    /// Check if present
    pub fn is_present(&self) -> bool {
        self.0 & 1 != 0
    }
    
    /// Get physical address
    pub fn phys_addr(&self) -> PhysicalAddress {
        PhysicalAddress::new((self.0 & 0x000F_FFFF_FFFF_F000) as usize)
    }
    
    /// Get flags
    pub fn flags(&self) -> u64 {
        self.0 & 0xFFF0_0000_0000_0FFF
    }
}

/// A page table (512 entries, 4KB)
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl PageTable {
    /// Create empty page table
    pub const fn empty() -> Self {
        Self {
            entries: [PageTableEntry::empty(); 512],
        }
    }
    
    /// Get entry at index
    pub fn entry(&self, index: usize) -> &PageTableEntry {
        &self.entries[index]
    }
    
    /// Get mutable entry at index
    pub fn entry_mut(&mut self, index: usize) -> &mut PageTableEntry {
        &mut self.entries[index]
    }
    
    /// Set entry at index
    pub fn set_entry(&mut self, index: usize, entry: PageTableEntry) {
        self.entries[index] = entry;
    }
}

/// User address space with its own page tables
pub struct UserAddressSpace {
    /// Physical address of PML4 (root page table)
    pml4_phys: PhysicalAddress,
    /// Virtual address of PML4 (for kernel access)
    pml4_virt: *mut PageTable,
    /// Allocated page tables (to free on drop)
    allocated_tables: Vec<*mut PageTable>,
    /// Allocated physical frames for user pages
    allocated_frames: Vec<PhysicalAddress>,
}

impl UserAddressSpace {
    /// Create a new user address space
    pub fn new() -> Result<Self, MemoryError> {
        // Allocate PML4
        let pml4 = Self::alloc_page_table()?;
        let pml4_phys = Self::virt_to_phys(pml4 as usize);
        
        log::debug!("Created user address space: PML4 @ {:#x} (phys: {:#x})", 
            pml4 as usize, pml4_phys.value());
        
        // Copy kernel mappings from current PML4
        // The upper half of the address space (entries 256-511) is for kernel
        unsafe {
            let current_pml4 = Self::get_current_pml4();
            for i in 256..512 {
                (*pml4).entries[i] = (*current_pml4).entries[i];
            }
        }
        
        Ok(Self {
            pml4_phys,
            pml4_virt: pml4,
            allocated_tables: vec![pml4],
            allocated_frames: Vec::new(),
        })
    }
    
    /// Get the CR3 value for this address space
    pub fn cr3(&self) -> u64 {
        self.pml4_phys.value() as u64
    }
    
    /// Map a virtual address to a physical frame with given flags
    pub fn map_page(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: UserPageFlags,
    ) -> Result<(), MemoryError> {
        let vaddr = virt.value();
        
        // Extract indices for each level
        let pml4_idx = (vaddr >> 39) & 0x1FF;
        let pdpt_idx = (vaddr >> 30) & 0x1FF;
        let pd_idx = (vaddr >> 21) & 0x1FF;
        let pt_idx = (vaddr >> 12) & 0x1FF;
        
        // Walk/create page tables
        let pml4 = self.pml4_virt;
        
        // Get or create PDPT
        let pdpt = unsafe { self.get_or_create_table(pml4, pml4_idx)? };
        
        // Get or create PD
        let pd = unsafe { self.get_or_create_table(pdpt, pdpt_idx)? };
        
        // Get or create PT
        let pt = unsafe { self.get_or_create_table(pd, pd_idx)? };
        
        // Set the final entry
        unsafe {
            let entry = PageTableEntry::new(phys, flags);
            (*pt).set_entry(pt_idx, entry);
        }
        
        // Invalidate TLB for this address
        Self::invalidate_tlb(virt);
        
        Ok(())
    }
    
    /// Map a range of pages
    pub fn map_range(
        &mut self,
        virt_start: VirtualAddress,
        size: usize,
        flags: UserPageFlags,
    ) -> Result<(), MemoryError> {
        let num_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        
        for i in 0..num_pages {
            let virt = VirtualAddress::new(virt_start.value() + i * PAGE_SIZE);
            
            // Allocate a physical frame
            let frame = self.alloc_frame()?;
            
            // Zero the frame
            unsafe {
                ptr::write_bytes(frame.value() as *mut u8, 0, PAGE_SIZE);
            }
            
            self.map_page(virt, frame, flags)?;
        }
        
        Ok(())
    }
    
    /// Map ELF segment data
    pub fn map_segment_data(
        &mut self,
        virt_start: VirtualAddress,
        data: &[u8],
        mem_size: usize,
        flags: UserPageFlags,
    ) -> Result<(), MemoryError> {
        let num_pages = (mem_size + PAGE_SIZE - 1) / PAGE_SIZE;
        
        for i in 0..num_pages {
            let virt = VirtualAddress::new(virt_start.value() + i * PAGE_SIZE);
            
            // Allocate a physical frame
            let frame = self.alloc_frame()?;
            
            // Copy data or zero
            let page_start = i * PAGE_SIZE;
            let page_end = core::cmp::min(page_start + PAGE_SIZE, data.len());
            
            unsafe {
                let frame_ptr = frame.value() as *mut u8;
                
                if page_start < data.len() {
                    // Copy file data
                    let copy_len = page_end - page_start;
                    ptr::copy_nonoverlapping(
                        data.as_ptr().add(page_start),
                        frame_ptr,
                        copy_len,
                    );
                    
                    // Zero the rest of the page (BSS)
                    if copy_len < PAGE_SIZE {
                        ptr::write_bytes(frame_ptr.add(copy_len), 0, PAGE_SIZE - copy_len);
                    }
                } else {
                    // Pure BSS - all zeros
                    ptr::write_bytes(frame_ptr, 0, PAGE_SIZE);
                }
            }
            
            self.map_page(virt, frame, flags)?;
        }
        
        Ok(())
    }
    
    /// Allocate a physical frame
    fn alloc_frame(&mut self) -> Result<PhysicalAddress, MemoryError> {
        // Use heap allocation and convert to physical address
        // This works because we're identity-mapped in kernel space
        let frame = alloc::boxed::Box::new([0u8; PAGE_SIZE]);
        let ptr = alloc::boxed::Box::into_raw(frame) as *mut u8;
        let phys = Self::virt_to_phys(ptr as usize);
        
        self.allocated_frames.push(phys);
        
        Ok(phys)
    }
    
    /// Get or create a page table at the given index
    unsafe fn get_or_create_table(
        &mut self,
        parent: *mut PageTable,
        index: usize,
    ) -> Result<*mut PageTable, MemoryError> {
        let entry = (*parent).entry(index);
        
        if entry.is_present() {
            // Table already exists
            let phys = entry.phys_addr();
            Ok(Self::phys_to_virt(phys) as *mut PageTable)
        } else {
            // Create new table
            let new_table = Self::alloc_page_table()?;
            self.allocated_tables.push(new_table);
            
            let phys = Self::virt_to_phys(new_table as usize);
            let entry = PageTableEntry::new(
                phys,
                UserPageFlags::empty().present().writable().user(),
            );
            (*parent).set_entry(index, entry);
            
            Ok(new_table)
        }
    }
    
    /// Allocate a page table (4KB aligned)
    fn alloc_page_table() -> Result<*mut PageTable, MemoryError> {
        // Allocate aligned memory
        let layout = core::alloc::Layout::from_size_align(
            core::mem::size_of::<PageTable>(),
            4096,
        ).map_err(|_| MemoryError::AlignmentError)?;
        
        let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err(MemoryError::OutOfMemory);
        }
        
        Ok(ptr as *mut PageTable)
    }
    
    /// Convert virtual to physical address (identity mapping assumed for kernel)
    fn virt_to_phys(virt: usize) -> PhysicalAddress {
        // In kernel space, we assume identity mapping or known offset
        // For heap allocations, the virtual address IS the physical address
        // in our simple setup
        PhysicalAddress::new(virt)
    }
    
    /// Convert physical to virtual address
    fn phys_to_virt(phys: PhysicalAddress) -> usize {
        phys.value()
    }
    
    /// Get current PML4 from CR3
    fn get_current_pml4() -> *mut PageTable {
        let cr3: u64;
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
        }
        (cr3 & 0x000F_FFFF_FFFF_F000) as *mut PageTable
    }
    
    /// Invalidate TLB entry
    fn invalidate_tlb(virt: VirtualAddress) {
        unsafe {
            core::arch::asm!(
                "invlpg [{}]",
                in(reg) virt.value(),
                options(nostack)
            );
        }
    }
    
    /// Switch to this address space (load CR3)
    pub unsafe fn activate(&self) {
        core::arch::asm!(
            "mov cr3, {}",
            in(reg) self.cr3(),
            options(nostack)
        );
    }
}

impl Drop for UserAddressSpace {
    fn drop(&mut self) {
        // Free allocated frames
        for phys in &self.allocated_frames {
            // Convert back to pointer and free
            unsafe {
                let ptr = phys.value() as *mut [u8; PAGE_SIZE];
                let _ = alloc::boxed::Box::from_raw(ptr);
            }
        }
        
        // Free page tables (except those copied from kernel)
        for table in &self.allocated_tables {
            unsafe {
                let layout = core::alloc::Layout::from_size_align(
                    core::mem::size_of::<PageTable>(),
                    4096,
                ).unwrap();
                alloc::alloc::dealloc(*table as *mut u8, layout);
            }
        }
    }
}

// Note: UserAddressSpace contains raw pointers and should not be sent between threads
// when active. The user is responsible for ensuring proper synchronization.
// We would use `impl !Send for UserAddressSpace {}` but that requires nightly features.
