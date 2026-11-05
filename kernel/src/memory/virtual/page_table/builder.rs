// kernel/src/memory/virtual/page_table/builder.rs
//
// Page table builder — incremental construction of a PML4.
// Fluent interface for kernel and user mappings at boot.
// Layer 0 — no external dependency except `spin`.

use crate::memory::core::{
    AllocError, AllocFlags, Frame, PageFlags, PhysAddr, VirtAddr, HUGE_PAGE_SIZE, PAGE_SIZE,
};
use crate::memory::virt::page_table::walker::{FrameAllocatorForWalk, PageTableWalker};
use crate::memory::virt::page_table::x86_64::{phys_to_table_mut, read_cr3};

// ─────────────────────────────────────────────────────────────────────────────
// PAGE TABLE BUILDER
// ─────────────────────────────────────────────────────────────────────────────

/// Builder allowing the construction of a complete page table.
pub struct PageTableBuilder<'a, A: FrameAllocatorForWalk> {
    pml4_phys: PhysAddr,
    alloc: &'a A,
    walker: PageTableWalker,
}

impl<'a, A: FrameAllocatorForWalk> PageTableBuilder<'a, A> {
    /// Creates a builder with a freshly allocated PML4.
    pub fn new(alloc: &'a A) -> Result<Self, AllocError> {
        let pml4_frame = alloc.alloc_frame(AllocFlags::ZEROED)?;
        let pml4_phys = pml4_frame.start_address();
        Ok(PageTableBuilder {
            pml4_phys,
            alloc,
            walker: PageTableWalker::new(pml4_phys),
        })
    }

    /// Creates a builder on an existing PML4.
    pub fn from_existing(pml4_phys: PhysAddr, alloc: &'a A) -> Self {
        PageTableBuilder {
            pml4_phys,
            alloc,
            walker: PageTableWalker::new(pml4_phys),
        }
    }

    /// Maps a single page.
    pub fn map_page(
        &mut self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
    ) -> Result<&mut Self, AllocError> {
        self.walker.map(virt, frame, flags, self.alloc)?;
        Ok(self)
    }

    /// Maps a contiguous physical range to a contiguous virtual range.
    pub fn map_range(
        &mut self,
        virt_start: VirtAddr,
        phys_start: PhysAddr,
        size_bytes: usize,
        flags: PageFlags,
    ) -> Result<&mut Self, AllocError> {
        let n_pages = (size_bytes + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..n_pages {
            let v = VirtAddr::new(virt_start.as_u64() + (i * PAGE_SIZE) as u64);
            let p = PhysAddr::new(phys_start.as_u64() + (i * PAGE_SIZE) as u64);
            self.walker
                .map(v, Frame::containing(p), flags, self.alloc)?;
        }
        Ok(self)
    }

    /// Maps the entire kernel physmap (PHYS_MAP_BASE → physical 0).
    /// `phys_size` is the total detected RAM size.
    pub fn map_physmap(&mut self, phys_size: u64) -> Result<&mut Self, AllocError> {
        let phys_map_base = crate::memory::core::layout::PHYS_MAP_BASE;
        let flags = PageFlags::KERNEL_DATA;

        let page_size = PAGE_SIZE as u64;
        let huge_size = HUGE_PAGE_SIZE as u64;
        let mut offset = 0u64;
        while offset < phys_size {
            let remaining = phys_size - offset;
            let virt = VirtAddr::new(phys_map_base.as_u64() + offset);
            let phys = PhysAddr::new(offset);
            if remaining >= huge_size
                && (virt.as_u64() & (huge_size - 1)) == 0
                && phys.is_aligned(huge_size)
            {
                self.walker.map_huge_2m(virt, phys, flags, self.alloc)?;
                offset = offset.saturating_add(huge_size);
            } else {
                self.walker
                    .map(virt, Frame::containing(phys), flags, self.alloc)?;
                offset = offset.saturating_add(page_size);
            }
        }
        Ok(self)
    }

    /// Clones kernel entries (PML4[256..512]) from the active PML4.
    /// Used to create user space sharing the kernel.
    ///
    /// The current kernel is still linked and executed in the low identity-map
    /// window (starting at 1 MiB). Userspace CR3s must therefore also
    /// contain the low kernel image in supervisor pages, otherwise the first
    /// `context_switch_asm` faults immediately after `mov cr3`.
    ///
    /// # Safety
    /// Must be called when the active PML4 is complete and valid.
    pub unsafe fn copy_kernel_entries(&mut self) -> Result<&mut Self, AllocError> {
        let current_pml4_phys = read_cr3();
        let current_pml4 = phys_to_table_mut(current_pml4_phys);
        let new_pml4 = phys_to_table_mut(self.pml4_phys);
        // Kernel entries: indices 256..512 (upper half of the 48-bit address space)
        for i in 256..512 {
            new_pml4[i] = current_pml4[i];
        }

        self.remap_low_kernel_identity()?;
        Ok(self)
    }

    /// Reinstalls supervisor mappings for the low kernel image in this PML4.
    ///
    /// The kernel still executes its syscall/IRQ paths in the low identity-map
    /// window. This primitive is deliberately idempotent: it rewrites
    /// the expected kernel PTEs without touching user segments placed
    /// outside the kernel image.
    pub fn remap_low_kernel_identity(&mut self) -> Result<&mut Self, AllocError> {
        self.map_low_kernel_identity()?;
        Ok(self)
    }

    #[cfg(target_os = "none")]
    fn map_low_kernel_identity(&mut self) -> Result<(), AllocError> {
        unsafe extern "C" {
            static __text_start: u8;
            static __text_end: u8;
            static __rodata_start: u8;
            static __rodata_end: u8;
            static __data_start: u8;
            static __data_end: u8;
            static __bss_start: u8;
            static __bss_end: u8;
            static __kernel_end: u8;
        }

        let image_start = crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR;
        let text_start = (&raw const __text_start) as u64;
        let text_end = (&raw const __text_end) as u64;
        let rodata_start = (&raw const __rodata_start) as u64;
        let rodata_end = (&raw const __rodata_end) as u64;
        let data_start = (&raw const __data_start) as u64;
        let data_end = (&raw const __data_end) as u64;
        let bss_start = (&raw const __bss_start) as u64;
        let bss_end = (&raw const __bss_end) as u64;
        let kernel_end = (&raw const __kernel_end) as u64;

        let kernel_ro = PageFlags::PRESENT | PageFlags::GLOBAL | PageFlags::NO_EXECUTE;
        let text_end_page = Self::page_align_up(text_end);
        let rodata_end_page = Self::page_align_up(rodata_end);
        let data_end_page = Self::page_align_up(data_end);
        let bss_end_page = Self::page_align_up(bss_end);

        // Header before .text mapped as data (non-executable, respecting W^X).
        self.map_identity_range(image_start, text_start, PageFlags::KERNEL_DATA)?;
        self.map_identity_range(text_start, text_end, PageFlags::KERNEL_CODE)?;
        self.map_identity_range(text_end_page, rodata_start, PageFlags::KERNEL_DATA)?;
        self.map_identity_range(rodata_start, rodata_end, kernel_ro)?;
        self.map_identity_range(rodata_end_page, data_start, PageFlags::KERNEL_DATA)?;
        self.map_identity_range(data_start, data_end, PageFlags::KERNEL_DATA)?;
        self.map_identity_range(data_end_page, bss_start, PageFlags::KERNEL_DATA)?;
        self.map_identity_range(bss_start, bss_end, PageFlags::KERNEL_DATA)?;

        if bss_end < kernel_end {
            self.map_identity_range(bss_end_page, kernel_end, PageFlags::KERNEL_DATA)?;
        }

        // The current kernel still uses the low MMIO windows inherited from
        // boot for LAPIC/IOAPIC, and some timer paths remain active under
        // userspace CR3 before a potential full KPTI switch.
        self.map_identity_range(0xFEC0_0000, 0xFEC0_1000, PageFlags::KERNEL_DMA)?;
        self.map_identity_range(0xFED0_0000, 0xFED0_1000, PageFlags::KERNEL_DMA)?;
        self.map_identity_range(0xFEE0_0000, 0xFEE0_1000, PageFlags::KERNEL_DMA)?;
        Ok(())
    }

    #[cfg(not(target_os = "none"))]
    fn map_low_kernel_identity(&mut self) -> Result<(), AllocError> {
        Ok(())
    }

    #[cfg(target_os = "none")]
    fn map_identity_range(
        &mut self,
        start: u64,
        end: u64,
        flags: PageFlags,
    ) -> Result<(), AllocError> {
        if end <= start {
            return Ok(());
        }

        let page_mask = PAGE_SIZE as u64 - 1;
        let mut addr = start & !page_mask;
        let end_aligned = end.saturating_add(page_mask) & !page_mask;
        while addr < end_aligned {
            let phys = PhysAddr::new(addr);
            self.map_page(VirtAddr::new(addr), Frame::containing(phys), flags)?;
            addr = addr.saturating_add(PAGE_SIZE as u64);
        }
        Ok(())
    }

    #[cfg(target_os = "none")]
    #[inline]
    fn page_align_up(addr: u64) -> u64 {
        let page_mask = PAGE_SIZE as u64 - 1;
        addr.saturating_add(page_mask) & !page_mask
    }

    /// Clears the user half of the PML4 (indices 0..256).
    ///
    /// # Safety
    /// `pml4_phys` points to a valid PML4.
    pub unsafe fn clear_user_entries(&mut self) -> &mut Self {
        let pml4 = phys_to_table_mut(self.pml4_phys);
        for i in 0..256 {
            pml4[i].clear();
        }
        self
    }

    /// Returns the physical address of the constructed PML4.
    pub fn pml4_phys(&self) -> PhysAddr {
        self.pml4_phys
    }

    /// Consumes the builder and returns the PML4.
    pub fn build(self) -> PhysAddr {
        self.pml4_phys
    }
}
