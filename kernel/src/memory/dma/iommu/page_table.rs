// kernel/src/memory/dma/iommu/page_table.rs
//
// IOMMU page tables (unified format for Intel VT-d / AMD-Vi on abstraction side).
// The concrete hardware implementation is in intel_vtd.rs / amd_iommu.rs.
// This file provides portable page tree logic.
//
// Format: 4 levels (L4/L3/L2/L1), 512 entries per level, 4096 bytes per table.
// LAYER 0 — no external dependencies.

use crate::memory::core::address::phys_to_virt;
use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{DmaError, IovaAddr};

// ─────────────────────────────────────────────────────────────────────────────
// IOMMU TABLE ENTRY
// ─────────────────────────────────────────────────────────────────────────────

/// Flags of an IOMMU table entry.
pub mod iommu_entry_flags {
    /// Present entry.
    pub const PRESENT: u64 = 1 << 0;
    /// Read allowed (device → memory).
    pub const READ: u64 = 1 << 1;
    /// Write allowed (memory → device).
    pub const WRITE: u64 = 1 << 2;
    /// Cache ignored (device coherency bypass).
    // FIX-IOMMU-SNP-01: the Snoop bit of VT-d PTE is bit 11 (DMA_PTE_SNP),
    // not bit 3 (reserved).
    pub const SNOOP: u64 = 1 << 11;
    /// Huge page (2MiB).
    pub const HUGE: u64 = 1 << 7;
    /// Accessed (hardware).
    pub const ACCESSED: u64 = 1 << 8;
    /// Dirty (hardware).
    pub const DIRTY: u64 = 1 << 9;
    /// Mask of physical address of the target frame.
    pub const PHYS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
}

/// An IOMMU table entry (64 bits).
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct IommuEntry(u64);

impl IommuEntry {
    pub const EMPTY: Self = IommuEntry(0);

    #[inline]
    pub fn is_present(self) -> bool {
        self.0 & iommu_entry_flags::PRESENT != 0
    }
    #[inline]
    pub fn is_huge(self) -> bool {
        self.0 & iommu_entry_flags::HUGE != 0
    }
    #[inline]
    pub fn phys(self) -> PhysAddr {
        PhysAddr::new(self.0 & iommu_entry_flags::PHYS_MASK)
    }
    /// Constructs an entry pointing to a lower-level table.
    #[inline]
    pub fn table_entry(phys: PhysAddr) -> Self {
        IommuEntry(
            phys.as_u64()
                | iommu_entry_flags::PRESENT
                | iommu_entry_flags::READ
                | iommu_entry_flags::WRITE,
        )
    }
    /// Constructs a leaf entry (4KiB page).
    #[inline]
    pub fn leaf_entry(phys: PhysAddr, read: bool, write: bool) -> Self {
        let mut flags = iommu_entry_flags::PRESENT;
        if read {
            flags |= iommu_entry_flags::READ;
        }
        if write {
            flags |= iommu_entry_flags::WRITE;
        }
        IommuEntry(phys.as_u64() | flags | iommu_entry_flags::SNOOP)
    }
    #[inline]
    pub fn raw(self) -> u64 {
        self.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IOMMU PAGE TABLE (512 ENTRIES, 4KiB)
// ─────────────────────────────────────────────────────────────────────────────

/// An IOMMU page table (4096 bytes, 512 64-bit entries).
#[repr(C, align(4096))]
pub struct IommuPageTable {
    entries: [IommuEntry; 512],
}

impl IommuPageTable {
    /// Returns the physical address of this table via the physmap.
    pub fn phys_addr(&self) -> PhysAddr {
        use crate::memory::core::address::virt_to_phys_physmap;
        let virt = crate::memory::core::types::VirtAddr::new(self as *const _ as u64);
        virt_to_phys_physmap(virt)
    }

    /// IOMMU index of IOVA at level `level` (0=L1, 3=L4).
    #[inline]
    pub fn index(iova: IovaAddr, level: u8) -> usize {
        ((iova.as_u64() >> (12 + level as u64 * 9)) & 0x1FF) as usize
    }

    /// Returns the entry at index `idx`.
    #[inline]
    pub fn entry(&self, idx: usize) -> IommuEntry {
        // SAFETY: idx < 512, valid memory read.
        unsafe { core::ptr::read_volatile(&self.entries[idx]) }
    }

    /// Writes the entry at index `idx` atomically.
    #[inline]
    pub fn set_entry(&mut self, idx: usize, entry: IommuEntry) {
        // SAFETY: idx < 512, write into mapped table.
        unsafe {
            core::ptr::write_volatile(&mut self.entries[idx], entry);
        }
    }

    /// Zeroes the entire table.
    pub fn zero(&mut self) {
        // SAFETY: The table is fully initialized.
        unsafe {
            core::ptr::write_bytes(self.entries.as_mut_ptr(), 0, 512);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// IOMMU TABLE WALKER
// ─────────────────────────────────────────────────────────────────────────────

/// Frame allocator trait for IOMMU tables (dependency inversion).
pub trait IommuFrameAlloc {
    fn alloc_table(&mut self) -> Option<PhysAddr>;
    fn free_table(&mut self, phys: PhysAddr);
}

/// Result of an IOMMU walk.
pub enum IommuWalkResult {
    /// Leaf mapping found.
    Leaf { phys: PhysAddr, huge: bool },
    /// Address not mapped.
    NotMapped,
    /// Allocation error during mapping.
    AllocError,
}

/// Configures an IOVA → PhysAddr mapping in the table rooted at `root`.
///
/// Creates intermediate tables if necessary via `alloc`.
///
/// # Safety
/// `root` must point to a valid IOMMU page table mapped in the physmap.
pub unsafe fn iommu_map<A: IommuFrameAlloc>(
    root: PhysAddr,
    iova: IovaAddr,
    phys: PhysAddr,
    read: bool,
    write: bool,
    alloc: &mut A,
) -> Result<(), DmaError> {
    let virt_root = phys_to_virt(root);
    let l4 = &mut *(virt_root.as_u64() as *mut IommuPageTable);

    let l4_idx = IommuPageTable::index(iova, 3);
    let l3_phys = ensure_table(l4, l4_idx, alloc)?;

    let l3 = &mut *(phys_to_virt(l3_phys).as_u64() as *mut IommuPageTable);
    let l3_idx = IommuPageTable::index(iova, 2);
    let l2_phys = ensure_table(l3, l3_idx, alloc)?;

    let l2 = &mut *(phys_to_virt(l2_phys).as_u64() as *mut IommuPageTable);
    let l2_idx = IommuPageTable::index(iova, 1);
    let l1_phys = ensure_table(l2, l2_idx, alloc)?;

    let l1 = &mut *(phys_to_virt(l1_phys).as_u64() as *mut IommuPageTable);
    let l1_idx = IommuPageTable::index(iova, 0);
    l1.set_entry(l1_idx, IommuEntry::leaf_entry(phys, read, write));
    Ok(())
}

/// Removes an IOVA mapping from the table.
///
/// # Safety
/// `root` must point to a valid IOMMU page table.
pub unsafe fn iommu_unmap(root: PhysAddr, iova: IovaAddr) -> Result<(), DmaError> {
    let l4 = &mut *(phys_to_virt(root).as_u64() as *mut IommuPageTable);
    let l4_idx = IommuPageTable::index(iova, 3);
    let e4 = l4.entry(l4_idx);
    if !e4.is_present() {
        return Err(DmaError::InvalidParams);
    }

    let l3 = &mut *(phys_to_virt(e4.phys()).as_u64() as *mut IommuPageTable);
    let l3_idx = IommuPageTable::index(iova, 2);
    let e3 = l3.entry(l3_idx);
    if !e3.is_present() {
        return Err(DmaError::InvalidParams);
    }

    let l2 = &mut *(phys_to_virt(e3.phys()).as_u64() as *mut IommuPageTable);
    let l2_idx = IommuPageTable::index(iova, 1);
    let e2 = l2.entry(l2_idx);
    if !e2.is_present() {
        return Err(DmaError::InvalidParams);
    }

    let l1 = &mut *(phys_to_virt(e2.phys()).as_u64() as *mut IommuPageTable);
    let l1_idx = IommuPageTable::index(iova, 0);
    l1.set_entry(l1_idx, IommuEntry::EMPTY);
    Ok(())
}

/// Walks the table read-only for `iova`.
///
/// # Safety
/// `root` must point to a valid table.
pub unsafe fn iommu_walk(root: PhysAddr, iova: IovaAddr) -> IommuWalkResult {
    let l4 = &*(phys_to_virt(root).as_u64() as *const IommuPageTable);
    let e4 = l4.entry(IommuPageTable::index(iova, 3));
    if !e4.is_present() {
        return IommuWalkResult::NotMapped;
    }

    let l3 = &*(phys_to_virt(e4.phys()).as_u64() as *const IommuPageTable);
    let e3 = l3.entry(IommuPageTable::index(iova, 2));
    if !e3.is_present() {
        return IommuWalkResult::NotMapped;
    }
    if e3.is_huge() {
        return IommuWalkResult::Leaf {
            phys: e3.phys(),
            huge: true,
        };
    }

    let l2 = &*(phys_to_virt(e3.phys()).as_u64() as *const IommuPageTable);
    let e2 = l2.entry(IommuPageTable::index(iova, 1));
    if !e2.is_present() {
        return IommuWalkResult::NotMapped;
    }

    let l1 = &*(phys_to_virt(e2.phys()).as_u64() as *const IommuPageTable);
    let e1 = l1.entry(IommuPageTable::index(iova, 0));
    if !e1.is_present() {
        return IommuWalkResult::NotMapped;
    }
    IommuWalkResult::Leaf {
        phys: e1.phys(),
        huge: false,
    }
}

/// Ensures that a lower-level table exists at entry `idx` of `parent`.
/// Creates it via `alloc` if absent.
unsafe fn ensure_table<A: IommuFrameAlloc>(
    parent: &mut IommuPageTable,
    idx: usize,
    alloc: &mut A,
) -> Result<PhysAddr, DmaError> {
    let entry = parent.entry(idx);
    if entry.is_present() {
        return Ok(entry.phys());
    }
    // Allocate a new table.
    let phys = alloc.alloc_table().ok_or(DmaError::OutOfMemory)?;
    // Zero the new table.
    let table = &mut *(phys_to_virt(phys).as_u64() as *mut IommuPageTable);
    table.zero();
    parent.set_entry(idx, IommuEntry::table_entry(phys));
    Ok(phys)
}
