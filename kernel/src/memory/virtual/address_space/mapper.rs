// kernel/src/memory/virtual/address_space/mapper.rs
//
// Address space mapper — high-level interface for map/unmap/remap.
// Handles TLB invalidations automatically after each modification.
// Layer 0 — no external dependencies except `spin`.

use crate::memory::core::{AllocError, Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::tlb::{flush_range, flush_single};
use crate::memory::virt::page_table::{FrameAllocatorForWalk, PageTableWalker, WalkResult};

// ─────────────────────────────────────────────────────────────────────────────
// MAPPER
// ─────────────────────────────────────────────────────────────────────────────

/// Maps virtual pages in a specific address space.
/// Wrapper around PageTableWalker with automatic TLB management.
pub struct Mapper<'a, A: FrameAllocatorForWalk> {
    walker: PageTableWalker,
    alloc: &'a A,
    /// Number of pages mapped since creation (for stats).
    mapped_count: u64,
    unmapped_count: u64,
}

impl<'a, A: FrameAllocatorForWalk> Mapper<'a, A> {
    /// Creates a mapper for the PML4 at `pml4_phys`.
    pub fn new(pml4_phys: PhysAddr, alloc: &'a A) -> Self {
        Mapper {
            walker: PageTableWalker::new(pml4_phys),
            alloc,
            mapped_count: 0,
            unmapped_count: 0,
        }
    }

    /// Maps a virtual page to a physical frame.
    ///
    /// Automatically invalidates local TLB for `virt`.
    pub fn map(
        &mut self,
        virt: VirtAddr,
        frame: Frame,
        flags: PageFlags,
    ) -> Result<(), AllocError> {
        self.walker.map(virt, frame, flags, self.alloc)?;
        // SAFETY: virt is a canonical address (validated by PageTableWalker).
        unsafe {
            flush_single(virt);
        }
        self.mapped_count += 1;
        Ok(())
    }

    /// Maps a contiguous physical range to a contiguous virtual range.
    pub fn map_range(
        &mut self,
        virt_start: VirtAddr,
        phys_start: PhysAddr,
        size_bytes: usize,
        flags: PageFlags,
    ) -> Result<(), AllocError> {
        let n = (size_bytes + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..n {
            let v = VirtAddr::new(virt_start.as_u64() + (i * PAGE_SIZE) as u64);
            let p = PhysAddr::new(phys_start.as_u64() + (i * PAGE_SIZE) as u64);
            self.walker
                .map(v, Frame::containing(p), flags, self.alloc)?;
            // SAFETY: canonical addresses.
            unsafe {
                flush_single(v);
            }
        }
        self.mapped_count += n as u64;
        Ok(())
    }

    /// Unmaps a virtual page.
    ///
    /// Returns previously mapped frame, or `None`.
    /// Invalidates local TLB.
    pub fn unmap(&mut self, virt: VirtAddr) -> Option<Frame> {
        let result = self.walker.unmap(virt);
        if result.is_some() {
            // SAFETY: canonical address.
            unsafe {
                flush_single(virt);
            }
            self.unmapped_count += 1;
        }
        result
    }

    /// Unmaps a range of pages.
    /// Returns the number of pages actually unmapped.
    pub fn unmap_range(&mut self, start: VirtAddr, size_bytes: usize) -> usize {
        let n = (size_bytes + PAGE_SIZE - 1) / PAGE_SIZE;
        let mut count = 0usize;
        for i in 0..n {
            let v = VirtAddr::new(start.as_u64() + (i * PAGE_SIZE) as u64);
            if self.walker.unmap(v).is_some() {
                count += 1;
            }
        }
        if count > 0 {
            // SAFETY: canonical address range.
            unsafe {
                flush_range(start, VirtAddr::new(start.as_u64() + size_bytes as u64));
            }
        }
        self.unmapped_count += count as u64;
        count
    }

    /// Modifies flags of an existing page.
    pub fn remap_flags(&mut self, virt: VirtAddr, new_flags: PageFlags) -> Result<(), AllocError> {
        self.walker.remap_flags(virt, new_flags)?;
        // SAFETY: canonical address.
        unsafe {
            flush_single(virt);
        }
        Ok(())
    }

    /// Checks whether an address is mapped.
    pub fn is_mapped(&self, virt: VirtAddr) -> bool {
        matches!(
            self.walker.walk_read(virt),
            WalkResult::Leaf { .. } | WalkResult::HugePage { .. }
        )
    }

    /// Translates a virtual address into a physical address.
    pub fn translate(&self, virt: VirtAddr) -> Option<PhysAddr> {
        match self.walker.walk_read(virt) {
            WalkResult::Leaf { entry, .. } => {
                let page_offset = virt.as_u64() & (PAGE_SIZE as u64 - 1);
                Some(PhysAddr::new(entry.phys_addr().as_u64() + page_offset))
            }
            WalkResult::HugePage { entry, level } => {
                let page_size = level.page_size() as u64;
                let offset = virt.as_u64() & (page_size - 1);
                Some(PhysAddr::new(entry.phys_addr().as_u64() + offset))
            }
            _ => None,
        }
    }

    /// Returns mapper statistics.
    pub fn stats(&self) -> (u64, u64) {
        (self.mapped_count, self.unmapped_count)
    }
}
