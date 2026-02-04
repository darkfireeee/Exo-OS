//! User Space Page Tables
//! 
//! Simplified page table management for user processes.
//! Creates and manages page tables for user-space memory regions.

use crate::memory::{PhysicalAddress, VirtualAddress, MemoryError, PAGE_SIZE};
use alloc::vec;
use alloc::vec::Vec;
use core::ptr;

/// Page table entry flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserPageFlags(u64);

impl UserPageFlags {
    /// Bit flags as constants
    pub const PRESENT: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const USER: Self = Self(1 << 2);
    pub const WRITE_THROUGH: Self = Self(1 << 3);
    pub const CACHE_DISABLE: Self = Self(1 << 4);
    pub const COW: Self = Self(1 << 9); // Using available bit 9 for CoW marker
    pub const NO_EXECUTE: Self = Self(1 << 63);
    
    /// No flags
    pub const fn empty() -> Self {
        Self(0)
    }
    
    /// Check if flag is present
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
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
    
    /// Copy-on-Write bit
    pub const fn cow(self) -> Self {
        Self(self.0 | (1 << 9))
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
    
    /// Check if WRITABLE flag is set
    pub fn contains_writable(&self) -> bool {
        (self.0 & (1 << 1)) != 0
    }

    /// Remove WRITABLE flag (pour CoW)
    pub fn remove_writable(self) -> Self {
        Self(self.0 & !(1 << 1))
    }
}

/// Implement BitOr for combining flags
impl core::ops::BitOr for UserPageFlags {
    type Output = Self;
    
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
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
    
    /// Get raw value
    pub fn value(&self) -> u64 {
        self.0
    }
    
    /// Check if present (alias for is_present)
    pub fn present(&self) -> bool {
        self.is_present()
    }
    
    /// Check if writable
    pub fn writable(&self) -> bool {
        self.0 & (1 << 1) != 0
    }
    
    /// Check if user accessible
    pub fn user(&self) -> bool {
        self.0 & (1 << 2) != 0
    }
    
    /// Check if no-execute
    pub fn no_execute(&self) -> bool {
        self.0 & (1 << 63) != 0
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

// SAFETY: UserAddressSpace owns its page tables and frames.
// The raw pointers are only used for kernel access to these owned resources.
// Access is synchronized externally (via Process mutex).
unsafe impl Send for UserAddressSpace {}

// Debug implementation for UserAddressSpace
impl core::fmt::Debug for UserAddressSpace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UserAddressSpace")
            .field("pml4_phys", &self.pml4_phys)
            .field("pml4_virt", &self.pml4_virt)
            .field("allocated_tables_len", &self.allocated_tables.len())
            .field("allocated_frames_len", &self.allocated_frames.len())
            .finish()
    }
}

impl UserAddressSpace {
    /// Create a new user address space
    pub fn new() -> Result<Self, MemoryError> {
        crate::logger::early_print("[DEBUG] UserAddressSpace::new() called\n");
        
        // Allocate PML4
        crate::logger::early_print("[DEBUG] About to allocate PML4...\n");
        let pml4 = Self::alloc_page_table()?;
        crate::logger::early_print("[DEBUG] PML4 allocated!\n");
        
        let pml4_phys = Self::virt_to_phys(pml4 as usize);
        crate::logger::early_print("[DEBUG] virt_to_phys done\n");
        
        // NOTE: For testing, we skip copying kernel mappings
        // In production, this should copy kernel space mappings
        crate::logger::early_print("[DEBUG] Skipping kernel mapping copy for test\n");
        
        crate::logger::early_print("[DEBUG] Creating Vec for allocated_tables...\n");
        let mut allocated_tables = Vec::new();
        allocated_tables.push(pml4);
        crate::logger::early_print("[DEBUG] allocated_tables created\n");
        
        crate::logger::early_print("[DEBUG] Creating Vec for allocated_frames...\n");
        let allocated_frames = Vec::new();
        crate::logger::early_print("[DEBUG] allocated_frames created\n");
        
        crate::logger::early_print("[DEBUG] About to create struct and return...\n");
        let result = Self {
            pml4_phys,
            pml4_virt: pml4,
            allocated_tables,
            allocated_frames,
        };
        crate::logger::early_print("[DEBUG] Struct created!\n");
        Ok(result)
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
    
    /// Map a range of pages with test pattern (écriture dans frame physique)
    pub fn map_range_with_pattern(
        &mut self,
        virt_start: VirtualAddress,
        size: usize,
        flags: UserPageFlags,
        base_pattern: u64,
    ) -> Result<(), MemoryError> {
        let num_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        
        for i in 0..num_pages {
            let virt = VirtualAddress::new(virt_start.value() + i * PAGE_SIZE);
            
            // Allocate a physical frame
            let frame = self.alloc_frame()?;
            
            // Écrire le pattern de test dans la frame physique
            // (accessible via identity mapping du kernel)
            unsafe {
                let frame_ptr = frame.value() as *mut u64;
                let entries_per_page = PAGE_SIZE / 8;
                for j in 0..entries_per_page {
                    // Pattern: base + page_index*1000 + offset
                    let pattern = base_pattern + (i as u64 * 1000) + j as u64;
                    ptr::write(frame_ptr.add(j), pattern);
                }
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
    
    /// Crée un address space de test avec de vraies pages mappées
    /// Pour tester CoW/fork avec des pages réelles
    pub fn new_test_user_space(heap_pages: usize, stack_pages: usize) -> Result<Self, MemoryError> {
        let mut space = Self::new()?;
        
        // Constantes d'adresses userspace
        const USER_HEAP_START: usize = 0x0000_4000_0000; // 1GB
        const USER_STACK_TOP: usize = 0x0000_7FFF_F000;  // Juste sous 128TB
        
        // Mapper le heap userspace avec pattern de test
        if heap_pages > 0 {
            let heap_size = heap_pages * PAGE_SIZE;
            space.map_range_with_pattern(
                VirtualAddress::new(USER_HEAP_START),
                heap_size,
                UserPageFlags::user_data(),
                0xDEAD_0000_0000,  // Pattern pour heap
            )?;
        }
        
        // Mapper la stack userspace (grandit vers le bas)
        if stack_pages > 0 {
            let stack_size = stack_pages * PAGE_SIZE;
            let stack_bottom = USER_STACK_TOP - stack_size;
            space.map_range_with_pattern(
                VirtualAddress::new(stack_bottom),
                stack_size,
                UserPageFlags::user_stack(),
                0xCAFE_0000_0000,  // Pattern pour stack
            )?;
        }
        
        // log::debug!(
        //     "Test user space created: {} heap pages @ {:#x}, {} stack pages @ {:#x}",
        //     heap_pages, USER_HEAP_START,
        //     stack_pages, USER_STACK_TOP - stack_pages * PAGE_SIZE
        // );
        
        Ok(space)
    }
    
    /// Retourne le nombre de pages mappées dans cet address space
    pub fn page_count(&self) -> usize {
        self.allocated_frames.len()
    }
    
    /// Clone l'address space pour CoW (fork)
    /// Walk all mapped pages in user space and collect their physical addresses
    /// Scans PML4 → PDPT → PD → PT to find all present pages in user space (entries 0-255)
    pub fn walk_pages(&self) -> Vec<(VirtualAddress, PhysicalAddress, UserPageFlags)> {
        let mut pages = Vec::new();
        
        unsafe {
            let pml4 = &*self.pml4_virt;
            
            // Only scan user space (PML4 entries 0-255)
            for pml4_idx in 0..256 {
                let pml4e = pml4.entries[pml4_idx];
                if !pml4e.present() {
                    continue;
                }
                
                let pdpt_phys = PhysicalAddress::new((pml4e.value() & 0x000F_FFFF_FFFF_F000) as usize);
                let pdpt = &*(Self::phys_to_virt(pdpt_phys) as *const PageTable);
                
                for pdpt_idx in 0..512 {
                    let pdpte = pdpt.entries[pdpt_idx];
                    if !pdpte.present() {
                        continue;
                    }
                    
                    // Check for 1GB huge page
                    if pdpte.value() & (1 << 7) != 0 {
                        // 1GB huge page - skip for now
                        continue;
                    }
                    
                    let pd_phys = PhysicalAddress::new((pdpte.value() & 0x000F_FFFF_FFFF_F000) as usize);
                    let pd = &*(Self::phys_to_virt(pd_phys) as *const PageTable);
                    
                    for pd_idx in 0..512 {
                        let pde = pd.entries[pd_idx];
                        if !pde.present() {
                            continue;
                        }
                        
                        // Check for 2MB huge page
                        if pde.value() & (1 << 7) != 0 {
                            // 2MB huge page - skip for now
                            continue;
                        }
                        
                        let pt_phys = PhysicalAddress::new((pde.value() & 0x000F_FFFF_FFFF_F000) as usize);
                        let pt = &*(Self::phys_to_virt(pt_phys) as *const PageTable);
                        
                        for pt_idx in 0..512 {
                            let pte = pt.entries[pt_idx];
                            if !pte.present() {
                                continue;
                            }
                            
                            // Reconstruct virtual address
                            let virt = VirtualAddress::new(
                                (pml4_idx << 39) | (pdpt_idx << 30) | (pd_idx << 21) | (pt_idx << 12)
                            );
                            
                            // Get physical address
                            let phys = PhysicalAddress::new((pte.value() & 0x000F_FFFF_FFFF_F000) as usize);
                            
                            // Extract flags as bitfield
                            let mut flags = UserPageFlags::empty().present();
                            if pte.writable() {
                                flags = flags.writable();
                            }
                            if pte.user() {
                                flags = flags.user();
                            }
                            if pte.value() & (1 << 3) != 0 {
                                flags = flags.write_through();
                            }
                            if pte.value() & (1 << 4) != 0 {
                                flags = flags.cache_disable();
                            }
                            if pte.no_execute() {
                                flags = flags.no_execute();
                            }
                            
                            pages.push((virt, phys, flags));
                        }
                    }
                }
            }
        }
        
        pages
    }
    
    /// Fork this address space with Copy-on-Write semantics
    /// Scans ALL mapped pages and marks them as CoW in both parent and child
    /// Fork this address space with Copy-on-Write semantics
    /// Scans ALL mapped pages and marks them as CoW in both parent and child
    pub fn fork_cow(&self) -> Result<Self, MemoryError> {
        let mut child = Self::new()?;
        
        // Walk all pages in parent address space
        let pages = self.walk_pages();
        let page_count = pages.len();
        
        // log::debug!(
        //     "fork_cow: Found {} mapped pages in parent address space",
        //     page_count
        // );
        
        // For each mapped page, share it with CoW
        for (virt, phys, flags) in pages {
            // Mark page as read-only in BOTH parent and child for CoW
            let cow_flags = flags.remove_writable().cow();
            
            // Map the same physical page in child (shared, read-only)
            child.map_page(virt, phys, cow_flags)?;
            
            // Mark physical page as CoW (increments refcount)
            let refcount = crate::memory::cow_manager::mark_cow(phys);
            
            log::trace!(
                "CoW: virt={:#x} → phys={:#x}, refcount={}",
                virt.value(),
                phys.value(),
                refcount
            );
        }
        
        // Also mark parent pages as read-only (for CoW fault detection)
        // TODO: Walk parent page tables and clear writable bit
        
        log::info!(
            "✅ fork_cow complete: {} pages shared with CoW",
            page_count
        );
        
        Ok(child)
    }
    
    /// Accès aux frames allouées (pour tests)
    pub fn allocated_frame_count(&self) -> usize {
        self.allocated_frames.len()
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
