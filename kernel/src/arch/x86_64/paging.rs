//! # arch/x86_64/paging.rs — x86_64 Page Tables (4-level, CR3)
//!
//! Manages page tables PML4 → PDPT → PD → PT for x86_64.
//! Supports KPTI (split user/kernel tables) via `spectre/kpti.rs`.
//!
//! ## Addressing Hierarchy
//! ```
//! Virtual address (48 bits):
//!   [47:39] → PML4 index (512 entries)
//!   [38:30] → PDPT index (512 entries)
//!   [29:21] → PD index   (512 entries)
//!   [20:12] → PT index   (512 entries)
//!   [11:0]  → Page offset (4 KiB)
//! ```
//!
//! ## Page Flags
//! Conforms to x86_64 PT entry bits

use core::sync::atomic::{AtomicUsize, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

pub const PAGE_SIZE: usize = 4096;
pub const HUGE_PAGE_2M: usize = 2 * 1024 * 1024;
pub const HUGE_PAGE_1G: usize = 1024 * 1024 * 1024;

pub const PAGE_TABLE_ENTRIES: usize = 512;

/// Mask for physical address in a PTE entry
pub const PTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Mask for bits [11:0] (flags)
pub const PTE_FLAGS_MASK: u64 = 0x0FFF;

// ── PTE Flags ─────────────────────────────────────────────────────────────────

/// Page present
pub const PTE_PRESENT: u64 = 1 << 0;
/// Page accessible for read/write (otherwise read-only)
pub const PTE_WRITABLE: u64 = 1 << 1;
/// Page accessible from user mode (Ring 3)
pub const PTE_USER: u64 = 1 << 2;
/// Write-Through caching
pub const PTE_WRITE_THROUGH: u64 = 1 << 3;
/// Cache Disabled
pub const PTE_CACHE_DISABLE: u64 = 1 << 4;
/// Page accessed (set by CPU on access)
pub const PTE_ACCESSED: u64 = 1 << 5;
/// Page dirty (set by CPU on write)
pub const PTE_DIRTY: u64 = 1 << 6;
/// Huge page (in PD = 2 MiB, in PDPT = 1 GiB)
pub const PTE_HUGE: u64 = 1 << 7;
/// Global page (not flushed on CR3 switch if PGE active)
pub const PTE_GLOBAL: u64 = 1 << 8;
/// Bit 9: used by Exo-OS for pending CoW
pub const PTE_COW: u64 = 1 << 9;
/// Bit 10: used for pinned shared memory
pub const PTE_SHM_PINNED: u64 = 1 << 10;
/// NX/XD bit — Non-Executable (bit 63)
pub const PTE_NO_EXEC: u64 = 1 << 63;

/// Flags for a normal kernel page (RW, global, no-exec data)
pub const PAGE_FLAGS_KERNEL_RW: u64 = PTE_PRESENT | PTE_WRITABLE | PTE_GLOBAL | PTE_NO_EXEC;

/// Flags for kernel code (RX, global)
pub const PAGE_FLAGS_KERNEL_RX: u64 = PTE_PRESENT | PTE_GLOBAL;

/// Flags for a read-only kernel page
pub const PAGE_FLAGS_KERNEL_RO: u64 = PTE_PRESENT | PTE_GLOBAL | PTE_NO_EXEC;

/// Flags for a userspace page (RW, user)
pub const PAGE_FLAGS_USER_RW: u64 = PTE_PRESENT | PTE_WRITABLE | PTE_USER | PTE_NO_EXEC;

/// Flags for userspace code (RX, user)
pub const PAGE_FLAGS_USER_RX: u64 = PTE_PRESENT | PTE_USER;

/// Flags for a CoW page (read-only pending copy)
pub const PAGE_FLAGS_USER_COW: u64 = PTE_PRESENT | PTE_USER | PTE_COW | PTE_NO_EXEC;

/// MMIO flags (cache disabled, RW, kernel)
pub const PAGE_FLAGS_MMIO: u64 = PTE_PRESENT | PTE_WRITABLE | PTE_CACHE_DISABLE | PTE_NO_EXEC;

// ── Page Table Entry (PTE) ────────────────────────────────────────────────────

/// 64-bit page table entry
#[derive(Debug, Clone, Copy, Default)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// Empty entry (not present)
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Constructs an entry with a physical address and flags
    pub const fn new(phys_addr: u64, flags: u64) -> Self {
        Self((phys_addr & PTE_ADDR_MASK) | (flags & !PTE_ADDR_MASK))
    }

    /// Returns the physical address (without flags)
    #[inline(always)]
    pub fn phys_addr(&self) -> u64 {
        self.0 & PTE_ADDR_MASK
    }

    /// Returns the flags
    #[inline(always)]
    pub fn flags(&self) -> u64 {
        self.0 & !PTE_ADDR_MASK
    }

    /// Page present?
    #[inline(always)]
    pub fn is_present(&self) -> bool {
        self.0 & PTE_PRESENT != 0
    }
    /// Page writable?
    #[inline(always)]
    pub fn is_writable(&self) -> bool {
        self.0 & PTE_WRITABLE != 0
    }
    /// Page user-accessible?
    #[inline(always)]
    pub fn is_user(&self) -> bool {
        self.0 & PTE_USER != 0
    }
    /// Huge page?
    #[inline(always)]
    pub fn is_huge(&self) -> bool {
        self.0 & PTE_HUGE != 0
    }
    /// No-execute?
    #[inline(always)]
    pub fn is_no_exec(&self) -> bool {
        self.0 & PTE_NO_EXEC != 0
    }
    /// CoW pending?
    #[inline(always)]
    pub fn is_cow(&self) -> bool {
        self.0 & PTE_COW != 0
    }
    /// SHM pinned?
    #[inline(always)]
    pub fn is_shm_pinned(&self) -> bool {
        self.0 & PTE_SHM_PINNED != 0
    }

    /// Adds the given flags
    #[inline(always)]
    pub fn set_flags(&mut self, flags: u64) {
        self.0 |= flags;
    }

    /// Clears the given flags
    #[inline(always)]
    pub fn clear_flags(&mut self, flags: u64) {
        self.0 &= !flags;
    }

    /// Raw value
    #[inline(always)]
    pub fn raw(&self) -> u64 {
        self.0
    }
}

// ── Page Table Structure ──────────────────────────────────────────────────────

/// A page table (512 × 8 bytes = 4 KiB, 4 KiB aligned)
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; PAGE_TABLE_ENTRIES],
}

impl PageTable {
    pub const fn empty() -> Self {
        Self {
            entries: [PageTableEntry::empty(); PAGE_TABLE_ENTRIES],
        }
    }

    /// Returns the entry at the given index
    #[inline(always)]
    pub fn entry(&self, idx: usize) -> &PageTableEntry {
        &self.entries[idx & 0x1FF]
    }

    /// Returns the mutable entry at the given index
    #[inline(always)]
    pub fn entry_mut(&mut self, idx: usize) -> &mut PageTableEntry {
        &mut self.entries[idx & 0x1FF]
    }

    /// Clears all entries (not present)
    pub fn clear(&mut self) {
        for e in self.entries.iter_mut() {
            *e = PageTableEntry::empty();
        }
    }
}

// ── Virtual Address Decomposition ────────────────────────────────────────────

/// Page table indices for a virtual address
#[derive(Debug, Clone, Copy)]
pub struct VirtAddrIndices {
    pub pml4_idx: usize,
    pub pdpt_idx: usize,
    pub pd_idx: usize,
    pub pt_idx: usize,
    pub offset: usize,
}

/// Decomposes a virtual address into page table indices
#[inline(always)]
pub fn decompose_virt_addr(va: u64) -> VirtAddrIndices {
    VirtAddrIndices {
        pml4_idx: ((va >> 39) & 0x1FF) as usize,
        pdpt_idx: ((va >> 30) & 0x1FF) as usize,
        pd_idx: ((va >> 21) & 0x1FF) as usize,
        pt_idx: ((va >> 12) & 0x1FF) as usize,
        offset: (va & 0xFFF) as usize,
    }
}

/// Constructs a virtual address from indices
#[inline(always)]
pub fn compose_virt_addr(pml4: usize, pdpt: usize, pd: usize, pt: usize, off: usize) -> u64 {
    let raw = ((pml4 & 0x1FF) as u64) << 39
        | ((pdpt & 0x1FF) as u64) << 30
        | ((pd & 0x1FF) as u64) << 21
        | ((pt & 0x1FF) as u64) << 12
        | (off as u64 & 0xFFF);
    // Sign-extend bit 47 for kernel addresses
    if raw & (1u64 << 47) != 0 {
        raw | 0xFFFF_0000_0000_0000
    } else {
        raw
    }
}

// ── Static Kernel PML4 (kernel root table) ───────────────────────────────────

/// Static kernel PML4 (used until the end of boot)
#[allow(dead_code)]
static mut KERNEL_PML4: PageTable = PageTable::empty();

/// Raw mapping: installs an entry in the existing hierarchy
///
/// # SAFETY
/// - `pml4` must point to the active PML4 or one under construction
/// - `phys_page` and intermediate tables must be valid frames
/// - Caller guarantees no data races on page tables
pub unsafe fn map_4k_page(
    pml4: *mut PageTable,
    virt_addr: u64,
    phys_addr: u64,
    flags: u64,
    alloc_page: impl Fn() -> Option<u64>,
) -> Result<(), PageTableError> {
    if virt_addr as usize & (PAGE_SIZE - 1) != 0 || phys_addr as usize & (PAGE_SIZE - 1) != 0 {
        return Err(PageTableError::InvalidAlignment);
    }
    let idx = decompose_virt_addr(virt_addr);
    let is_user_mapping = flags & PTE_USER != 0;

    // PML4 → PDPT
    // SAFETY: `pml4` is a valid pointer passed by caller (unsafe fn) to the
    // active PML4 table. 4 KiB alignment guaranteed. Exclusive access guaranteed by caller.
    let pml4 = unsafe { &mut *pml4 };
    let pdpt = get_or_create_subtable(pml4, idx.pml4_idx, is_user_mapping, &alloc_page)?;

    // PDPT → PD
    let pd = get_or_create_subtable(pdpt, idx.pdpt_idx, is_user_mapping, &alloc_page)?;

    // PD → PT
    let pt = get_or_create_subtable(pd, idx.pd_idx, is_user_mapping, &alloc_page)?;

    // PT → physical page
    if pt.entry(idx.pt_idx).is_present() {
        return Err(PageTableError::AlreadyMapped);
    }
    *pt.entry_mut(idx.pt_idx) = PageTableEntry::new(phys_addr, flags);

    PAGE_MAP_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Unmaps a 4 KiB page
///
/// # SAFETY
/// `pml4` is valid.
pub unsafe fn unmap_4k_page(pml4: *mut PageTable, virt_addr: u64) -> Result<u64, PageTableError> {
    if virt_addr as usize & (PAGE_SIZE - 1) != 0 {
        return Err(PageTableError::InvalidAlignment);
    }

    let idx = decompose_virt_addr(virt_addr);
    // SAFETY: `pml4` is a valid pointer (unsafe fn), 4 KiB aligned, exclusive access.
    let pml4 = unsafe { &mut *pml4 };

    let e_pml4 = pml4.entry(idx.pml4_idx);
    if !e_pml4.is_present() {
        return Err(PageTableError::NotMapped);
    }

    // SAFETY: e_pml4 is present — phys_addr() points to a valid PageTable, 4 KiB aligned.
    let pdpt = unsafe { &mut *(e_pml4.phys_addr() as *mut PageTable) };
    let e_pdpt = pdpt.entry(idx.pdpt_idx);
    if !e_pdpt.is_present() {
        return Err(PageTableError::NotMapped);
    }
    if e_pdpt.is_huge() {
        return Err(PageTableError::HugePageConflict);
    }

    // SAFETY: e_pdpt is present — same invariant as above.
    let pd = unsafe { &mut *(e_pdpt.phys_addr() as *mut PageTable) };
    let e_pd = pd.entry(idx.pd_idx);
    if !e_pd.is_present() {
        return Err(PageTableError::NotMapped);
    }
    if e_pd.is_huge() {
        return Err(PageTableError::HugePageConflict);
    }

    // SAFETY: e_pd is present and non-huge — same invariant.
    let pt = unsafe { &mut *(e_pd.phys_addr() as *mut PageTable) };
    let entry = pt.entry_mut(idx.pt_idx);
    if !entry.is_present() {
        return Err(PageTableError::NotMapped);
    }

    let phys = entry.phys_addr();
    *entry = PageTableEntry::empty();

    // Invalidate TLB locally
    super::invlpg(virt_addr);

    PAGE_UNMAP_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(phys)
}

/// Translates a virtual address to physical (page walk)
///
/// # SAFETY
/// `pml4` is valid. The hierarchy must not be modified during the walk.
pub unsafe fn translate_virt(pml4: *const PageTable, virt_addr: u64) -> Option<u64> {
    let idx = decompose_virt_addr(virt_addr);
    // SAFETY: `pml4` is a valid pointer passed by caller (unsafe fn), 4 KiB aligned.
    // The table hierarchy is not modified during this page walk.
    let pml4 = unsafe { &*pml4 };

    let e_pml4 = pml4.entry(idx.pml4_idx);
    if !e_pml4.is_present() {
        return None;
    }

    // SAFETY: e_pml4 is present — phys_addr() is a valid PageTable, 4 KiB aligned.
    let pdpt = unsafe { &*(e_pml4.phys_addr() as *const PageTable) };
    let e_pdpt = pdpt.entry(idx.pdpt_idx);
    if !e_pdpt.is_present() {
        return None;
    }

    // Huge page 1 GiB?
    if e_pdpt.is_huge() {
        let base = e_pdpt.phys_addr() & !(HUGE_PAGE_1G as u64 - 1);
        let off = virt_addr & (HUGE_PAGE_1G as u64 - 1);
        return Some(base | off);
    }

    // SAFETY: e_pdpt is present and non-huge — phys_addr() is a valid PageTable.
    let pd = unsafe { &*(e_pdpt.phys_addr() as *const PageTable) };
    let e_pd = pd.entry(idx.pd_idx);
    if !e_pd.is_present() {
        return None;
    }

    // Huge page 2 MiB?
    if e_pd.is_huge() {
        let base = e_pd.phys_addr() & !(HUGE_PAGE_2M as u64 - 1);
        let off = virt_addr & (HUGE_PAGE_2M as u64 - 1);
        return Some(base | off);
    }

    // SAFETY: e_pd is present and non-huge — phys_addr() is a valid PageTable.
    let pt = unsafe { &*(e_pd.phys_addr() as *const PageTable) };
    let e_pt = pt.entry(idx.pt_idx);
    if !e_pt.is_present() {
        return None;
    }

    Some(e_pt.phys_addr() | idx.offset as u64)
}

// ── Internal Helpers ─────────────────────────────────────────────────────────

fn get_or_create_subtable<'a>(
    parent: &'a mut PageTable,
    idx: usize,
    is_user_mapping: bool,
    alloc_page: &impl Fn() -> Option<u64>,
) -> Result<&'a mut PageTable, PageTableError> {
    let entry = parent.entry_mut(idx);
    if !entry.is_present() {
        let phys = alloc_page().ok_or(PageTableError::OutOfMemory)?;
        let mut flags = PTE_PRESENT | PTE_WRITABLE;
        if is_user_mapping {
            flags |= PTE_USER;
        }
        *entry = PageTableEntry::new(phys, flags);

        // Initialize subtable to zero
        // SAFETY: phys is a freshly allocated frame — no previous contents
        unsafe {
            let ptr = phys as *mut PageTable;
            (*ptr).clear();
        }
    } else if is_user_mapping && !entry.is_user() {
        entry.set_flags(PTE_USER);
    }
    // SAFETY: the entry is present and points to a valid PageTable
    Ok(unsafe { &mut *(entry.phys_addr() as *mut PageTable) })
}

// ── CR3 and TLB ──────────────────────────────────────────────────────────────

/// Loads CR3 (switch page tables)
///
/// # SAFETY
/// `pml4_phys` must point to a valid PML4 aligned to 4 KiB.
/// The return address must be mapped in the new table.
#[inline(always)]
pub unsafe fn switch_page_table(pml4_phys: u64) {
    // SAFETY: delegated to caller
    unsafe {
        super::write_cr3(pml4_phys);
    }
}

/// Full TLB flush (all non-global entries)
#[inline(always)]
pub fn flush_tlb() {
    let cr3 = super::read_cr3();
    // SAFETY: rewriting the same value = standard TLB flush
    unsafe {
        super::write_cr3(cr3);
    }
}

/// Flushes TLB for a virtual address
#[inline(always)]
pub fn flush_tlb_page(virt: u64) {
    super::invlpg(virt);
}

/// Full global TLB flush (includes global pages, resets CR4.PGE)
pub fn flush_tlb_global() {
    let cr4 = super::read_cr4();
    const CR4_PGE: u64 = 1 << 7;
    // SAFETY: toggling CR4.PGE is the documented method to flush global pages
    unsafe {
        super::write_cr4(cr4 & !CR4_PGE);
        super::write_cr4(cr4 | CR4_PGE);
    }
}

// ── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableError {
    AlreadyMapped,
    OutOfMemory,
    InvalidAlignment,
    NotMapped,
    HugePageConflict,
}

// ── Instrumentation ───────────────────────────────────────────────────────────

static PAGE_MAP_COUNT: AtomicUsize = AtomicUsize::new(0);
static PAGE_UNMAP_COUNT: AtomicUsize = AtomicUsize::new(0);
static PAGE_FAULT_COUNT: AtomicUsize = AtomicUsize::new(0);
static TLB_SHOOTDOWN_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn page_map_count() -> usize {
    PAGE_MAP_COUNT.load(Ordering::Relaxed)
}
pub fn page_unmap_count() -> usize {
    PAGE_UNMAP_COUNT.load(Ordering::Relaxed)
}
pub fn page_fault_count() -> usize {
    PAGE_FAULT_COUNT.load(Ordering::Relaxed)
}
pub fn inc_page_fault() {
    PAGE_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);
}
pub fn inc_tlb_shootdown() {
    TLB_SHOOTDOWN_COUNT.fetch_add(1, Ordering::Relaxed);
}
pub fn tlb_shootdown_count() -> usize {
    TLB_SHOOTDOWN_COUNT.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::boxed::Box;

    #[test]
    fn unmap_4k_page_reports_invalid_alignment() {
        let mut pml4 = Box::new(PageTable::empty());

        let err = unsafe { unmap_4k_page(&mut *pml4, 0x1001) }.unwrap_err();

        assert_eq!(err, PageTableError::InvalidAlignment);
    }

    #[test]
    fn unmap_4k_page_reports_not_mapped_for_empty_tree() {
        let mut pml4 = Box::new(PageTable::empty());

        let err = unsafe { unmap_4k_page(&mut *pml4, 0x4000) }.unwrap_err();

        assert_eq!(err, PageTableError::NotMapped);
    }

    #[test]
    fn unmap_4k_page_reports_huge_page_conflict_at_pdpt() {
        let mut pml4 = Box::new(PageTable::empty());
        let mut pdpt = Box::new(PageTable::empty());
        let pdpt_addr = (&mut *pdpt as *mut PageTable) as u64;

        *pml4.entry_mut(0) = PageTableEntry::new(pdpt_addr, PTE_PRESENT | PTE_WRITABLE);
        *pdpt.entry_mut(0) = PageTableEntry::new(0, PTE_PRESENT | PTE_HUGE);

        let err = unsafe { unmap_4k_page(&mut *pml4, 0) }.unwrap_err();

        assert_eq!(err, PageTableError::HugePageConflict);
    }
}
