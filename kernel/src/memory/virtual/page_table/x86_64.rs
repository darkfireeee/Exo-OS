// kernel/src/memory/virtual/page_table/x86_64.rs
//
// x86_64 4-level page tables (PML4 → PDPT → PD → PT).
// Complete implementation with flags, entries, and access helpers.
// Layer 0 — no external dependency except `spin`.

use core::ops::{Index, IndexMut};

use crate::memory::core::{Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE, PHYS_MAP_BASE};

// ─────────────────────────────────────────────────────────────────────────────
// PAGE TABLE ENTRY (PageTableEntry)
// ─────────────────────────────────────────────────────────────────────────────

/// x86_64 page table entry (64 bits).
///
/// Bits 0-11: flags
/// Bits 12-51: physical frame address (aligned to 4 KiB)
/// Bits 52-62: upper flags (available)
/// Bit  63  : NX (No-Execute)
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// Null entry (not present).
    pub const EMPTY: PageTableEntry = PageTableEntry(0);

    // ── Flag bits ────────────────────────────────────────────────────────────
    pub const FLAG_PRESENT: u64 = 1 << 0;
    pub const FLAG_WRITABLE: u64 = 1 << 1;
    pub const FLAG_USER: u64 = 1 << 2;
    pub const FLAG_WRITE_THROUGH: u64 = 1 << 3;
    pub const FLAG_NO_CACHE: u64 = 1 << 4;
    pub const FLAG_ACCESSED: u64 = 1 << 5;
    pub const FLAG_DIRTY: u64 = 1 << 6;
    pub const FLAG_HUGE_PAGE: u64 = 1 << 7; // PSE bit in PD/PDPT
    pub const FLAG_PAT_L1: u64 = 1 << 7; // PAT bit in a 4 KiB PT leaf
    pub const FLAG_GLOBAL: u64 = 1 << 8;
    pub const FLAG_COW: u64 = 1 << 9; // Available to OS
    pub const FLAG_PINNED: u64 = 1 << 10; // Available to OS
    pub const FLAG_NO_EXECUTE: u64 = 1 << 63;

    /// Physical address bitmask.
    const PHYS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    #[inline]
    pub fn new(frame: Frame, flags: u64) -> Self {
        PageTableEntry(frame.start_address().as_u64() | flags)
    }

    #[inline]
    pub fn from_raw(raw: u64) -> Self {
        PageTableEntry(raw)
    }
    #[inline]
    pub fn raw(self) -> u64 {
        self.0
    }
    #[inline]
    pub fn is_present(self) -> bool {
        self.0 & Self::FLAG_PRESENT != 0
    }
    #[inline]
    pub fn is_writable(self) -> bool {
        self.0 & Self::FLAG_WRITABLE != 0
    }
    #[inline]
    pub fn is_user(self) -> bool {
        self.0 & Self::FLAG_USER != 0
    }
    #[inline]
    pub fn is_huge(self) -> bool {
        self.0 & Self::FLAG_HUGE_PAGE != 0
    }
    #[inline]
    pub fn is_global(self) -> bool {
        self.0 & Self::FLAG_GLOBAL != 0
    }
    #[inline]
    pub fn is_no_execute(self) -> bool {
        self.0 & Self::FLAG_NO_EXECUTE != 0
    }
    #[inline]
    pub fn is_cow(self) -> bool {
        self.0 & Self::FLAG_COW != 0
    }
    #[inline]
    pub fn is_pinned(self) -> bool {
        self.0 & Self::FLAG_PINNED != 0
    }
    #[inline]
    pub fn is_accessed(self) -> bool {
        self.0 & Self::FLAG_ACCESSED != 0
    }
    #[inline]
    pub fn is_dirty(self) -> bool {
        self.0 & Self::FLAG_DIRTY != 0
    }

    /// Physical address encoded in this entry.
    #[inline]
    pub fn phys_addr(self) -> PhysAddr {
        PhysAddr::new(self.0 & Self::PHYS_MASK)
    }

    /// Physical frame pointed to by this entry.
    #[inline]
    pub fn frame(self) -> Option<Frame> {
        if self.is_present() {
            Some(Frame::containing(self.phys_addr()))
        } else {
            None
        }
    }

    /// Enables a flag.
    #[inline]
    pub fn set_flag(&mut self, flag: u64) {
        self.0 |= flag;
    }
    /// Disables a flag.
    #[inline]
    pub fn clear_flag(&mut self, flag: u64) {
        self.0 &= !flag;
    }

    /// Clears the entry completely.
    #[inline]
    pub fn clear(&mut self) {
        self.0 = 0;
    }

    /// Converts kernel PageFlags into x86_64 entry bits.
    pub fn from_page_flags(frame: Frame, flags: PageFlags) -> Self {
        let mut raw = frame.start_address().as_u64();
        if flags.contains(PageFlags::PRESENT) {
            raw |= Self::FLAG_PRESENT;
        }
        if flags.contains(PageFlags::WRITABLE) {
            raw |= Self::FLAG_WRITABLE;
        }
        if flags.contains(PageFlags::USER) {
            raw |= Self::FLAG_USER;
        }
        if flags.contains(PageFlags::WRITE_THROUGH) {
            raw |= Self::FLAG_WRITE_THROUGH;
        }
        if flags.contains(PageFlags::NO_CACHE) {
            raw |= Self::FLAG_NO_CACHE;
        }
        if flags.contains(PageFlags::WRITE_COMBINING) {
            // PAT index 7 = PAT|PCD|PWT; `configure_pat()` makes index 7 WC.
            raw |= Self::FLAG_PAT_L1 | Self::FLAG_NO_CACHE | Self::FLAG_WRITE_THROUGH;
        }
        if flags.contains(PageFlags::GLOBAL) {
            raw |= Self::FLAG_GLOBAL;
        }
        if flags.contains(PageFlags::NO_EXECUTE) {
            raw |= Self::FLAG_NO_EXECUTE;
        }
        if flags.contains(PageFlags::COW) {
            raw |= Self::FLAG_COW;
        }
        if flags.contains(PageFlags::PINNED) {
            raw |= Self::FLAG_PINNED;
        }
        if flags.contains(PageFlags::HUGE_PAGE) {
            raw |= Self::FLAG_HUGE_PAGE;
        }
        PageTableEntry(raw)
    }

    /// Extracts PageFlags from an entry.
    pub fn to_page_flags(self) -> PageFlags {
        let mut f = PageFlags::EMPTY;
        if self.0 & Self::FLAG_PRESENT != 0 {
            f = f.set(PageFlags::PRESENT);
        }
        if self.0 & Self::FLAG_WRITABLE != 0 {
            f = f.set(PageFlags::WRITABLE);
        }
        if self.0 & Self::FLAG_USER != 0 {
            f = f.set(PageFlags::USER);
        }
        if self.0 & Self::FLAG_WRITE_THROUGH != 0 {
            f = f.set(PageFlags::WRITE_THROUGH);
        }
        if self.0 & Self::FLAG_NO_CACHE != 0 {
            f = f.set(PageFlags::NO_CACHE);
        }
        if self.0 & (Self::FLAG_PAT_L1 | Self::FLAG_NO_CACHE | Self::FLAG_WRITE_THROUGH)
            == (Self::FLAG_PAT_L1 | Self::FLAG_NO_CACHE | Self::FLAG_WRITE_THROUGH)
        {
            f = f.set(PageFlags::WRITE_COMBINING);
        }
        if self.0 & Self::FLAG_ACCESSED != 0 {
            f = f.set(PageFlags::ACCESSED);
        }
        if self.0 & Self::FLAG_DIRTY != 0 {
            f = f.set(PageFlags::DIRTY);
        }
        if self.0 & Self::FLAG_HUGE_PAGE != 0 {
            f = f.set(PageFlags::HUGE_PAGE);
        }
        if self.0 & Self::FLAG_GLOBAL != 0 {
            f = f.set(PageFlags::GLOBAL);
        }
        if self.0 & Self::FLAG_NO_EXECUTE != 0 {
            f = f.set(PageFlags::NO_EXECUTE);
        }
        if self.0 & Self::FLAG_COW != 0 {
            f = f.set(PageFlags::COW);
        }
        if self.0 & Self::FLAG_PINNED != 0 {
            f = f.set(PageFlags::PINNED);
        }
        f
    }
}

impl core::fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "PTE({:#018x} phys={:#x} flags={:?})",
            self.0,
            self.0 & Self::PHYS_MASK,
            self.to_page_flags()
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PAGE TABLE (512 entries)
// ─────────────────────────────────────────────────────────────────────────────

/// An x86_64 page table (exactly one page = 4 KiB = 512 × 8 bytes).
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; 512],
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PageTable {
    /// Creates an empty table (all entries = 0).
    pub const fn new() -> Self {
        PageTable {
            entries: [PageTableEntry::EMPTY; 512],
        }
    }

    /// Clears all entries.
    pub fn zero(&mut self) {
        for e in &mut self.entries {
            e.clear();
        }
    }

    /// Clears user entries (indices 0..256).
    pub fn zero_user_half(&mut self) {
        for e in &mut self.entries[0..256] {
            e.clear();
        }
    }

    #[inline]
    pub fn iter(&self) -> core::slice::Iter<'_, PageTableEntry> {
        self.entries.iter()
    }
    #[inline]
    pub fn iter_mut(&mut self) -> core::slice::IterMut<'_, PageTableEntry> {
        self.entries.iter_mut()
    }
    #[inline]
    pub fn len(&self) -> usize {
        512
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Physical address of this table (via physmap).
    pub fn phys_addr(&self) -> PhysAddr {
        // SAFETY: self is in physical memory mapped via PHYS_MAP_BASE.
        let virt = self as *const _ as u64;
        if virt >= PHYS_MAP_BASE.as_u64() {
            PhysAddr::new(virt - PHYS_MAP_BASE.as_u64())
        } else {
            // Fallback for tables in the kernel .bss segment.
            PhysAddr::new(
                virt - crate::memory::core::layout::KERNEL_START.as_u64() + KERNEL_PHYS_BASE,
            )
        }
    }
}

/// Physical base of the kernel (assumed 1 MiB for standard x86_64).
const KERNEL_PHYS_BASE: u64 = 0x0010_0000;

impl Index<usize> for PageTable {
    type Output = PageTableEntry;
    fn index(&self, i: usize) -> &Self::Output {
        &self.entries[i]
    }
}
impl IndexMut<usize> for PageTable {
    fn index_mut(&mut self, i: usize) -> &mut Self::Output {
        &mut self.entries[i]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE LEVELS
// ─────────────────────────────────────────────────────────────────────────────

/// Level of a page table (4=PML4, 3=PDPT, 2=PD, 1=PT).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PageTableLevel {
    /// Page Table (level 1).
    L1 = 1,
    /// Page Directory (level 2).
    L2 = 2,
    /// Page Directory Pointer Table (level 3).
    L3 = 3,
    /// PML4 (level 4).
    L4 = 4,
}

impl PageTableLevel {
    /// Returns the index in VirtAddr for this level.
    #[inline]
    pub fn index_of(self, addr: VirtAddr) -> usize {
        match self {
            PageTableLevel::L4 => addr.p4_index(),
            PageTableLevel::L3 => addr.p3_index(),
            PageTableLevel::L2 => addr.p2_index(),
            PageTableLevel::L1 => addr.p1_index(),
        }
    }

    /// Lower level.
    pub fn descend(self) -> Option<PageTableLevel> {
        match self {
            PageTableLevel::L4 => Some(PageTableLevel::L3),
            PageTableLevel::L3 => Some(PageTableLevel::L2),
            PageTableLevel::L2 => Some(PageTableLevel::L1),
            PageTableLevel::L1 => None,
        }
    }

    /// Page size mapped at this level (huge page).
    pub fn page_size(self) -> usize {
        match self {
            PageTableLevel::L4 => 512 * 1024 * 1024 * 1024, // 512 GiB (unused)
            PageTableLevel::L3 => 1024 * 1024 * 1024,       // 1 GiB
            PageTableLevel::L2 => 2 * 1024 * 1024,          // 2 MiB
            PageTableLevel::L1 => PAGE_SIZE,                // 4 KiB
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPERS: physmap → page table
// ─────────────────────────────────────────────────────────────────────────────

/// Returns a pointer to the physical page table via physmap.
///
/// # Safety
/// `phys` must point to a valid PageTable and physmap must
/// be initialized (PHYS_MAP_BASE mapped).
#[inline]
pub unsafe fn phys_to_table(phys: PhysAddr) -> *mut PageTable {
    (PHYS_MAP_BASE.as_u64() + phys.as_u64()) as *mut PageTable
}

/// Returns a reference to the physical page table via physmap.
///
/// # Safety
/// Same conditions as phys_to_table. Lifetime is tied to
/// physmap (static).
#[inline]
pub unsafe fn phys_to_table_ref<'a>(phys: PhysAddr) -> &'a PageTable {
    &*(phys_to_table(phys))
}

/// Returns a mutable reference to the physical page table.
///
/// # Safety
/// Same conditions as phys_to_table. No other mutable reference
/// must exist simultaneously.
#[inline]
pub unsafe fn phys_to_table_mut<'a>(phys: PhysAddr) -> &'a mut PageTable {
    &mut *(phys_to_table(phys))
}

// ─────────────────────────────────────────────────────────────────────────────
// CR3 REGISTER
// ─────────────────────────────────────────────────────────────────────────────

/// Reads the physical address of the active PML4 from CR3.
#[inline]
pub fn read_cr3() -> PhysAddr {
    let val: u64;
    // SAFETY: reading CR3, standard x86_64 operation at ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) val, options(nomem, nostack));
    }
    PhysAddr::new(val & !0xFFF)
}

/// Loads a new PML4 into CR3 (invalidates the entire global TLB).
///
/// # Safety
/// `pml4` must point to a valid PML4. Invalid entries
/// will trigger an immediate #PF or #GP.
#[inline]
pub unsafe fn write_cr3(pml4: PhysAddr) {
    let pcid_bits = read_cr3().as_u64() & 0xFFF; // Preserve PCID bits
    core::arch::asm!(
        "mov cr3, {}",
        in(reg) pml4.as_u64() | pcid_bits,
        options(nomem, nostack),
    );
}

/// Invalidates a single TLB entry for the given virtual address.
///
/// # Safety
/// addr must be a canonical x86_64 address.
#[inline]
pub unsafe fn invlpg(addr: VirtAddr) {
    core::arch::asm!(
        "invlpg [{}]",
        in(reg) addr.as_u64(),
        options(nostack, preserves_flags),
    );
}
