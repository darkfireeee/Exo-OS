// kernel/src/memory/core/types.rs
//
// Fundamental types of the memory module — Exo-OS Layer 0
// No external dependencies to the kernel.
// All types are statically tested for size/alignment.

use super::constants::{HUGE_PAGE_SIZE, PAGE_MASK, PAGE_SHIFT};
use core::fmt;
use core::ops::{Add, AddAssign, BitAnd, BitOr, Not, Sub, SubAssign};

// ─────────────────────────────────────────────────────────────────────────────
// PHYSADDR — Physical address
// ─────────────────────────────────────────────────────────────────────────────

/// 64-bit physical address — opaque, cannot be dereferenced directly.
/// Always canonical (bits 48..63 = 0 on x86_64 without 5-level paging).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysAddr(u64);

impl PhysAddr {
    /// Creates a PhysAddr from a raw value.
    /// Panics (debug) if non-canonical.
    #[inline(always)]
    pub const fn new(addr: u64) -> Self {
        // In no_std, we cannot panic in const — we accept silently
        // Validation occurs upon use via is_canonical()
        PhysAddr(addr)
    }

    /// Creates a PhysAddr without any verification (unsafe).
    ///
    /// # Safety
    /// Caller guarantees that the address is physically valid
    /// and within the hardware's addressable range.
    #[inline(always)]
    pub const unsafe fn new_unchecked(addr: u64) -> Self {
        PhysAddr(addr)
    }

    /// Returns the raw value of the address.
    #[inline(always)]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns the raw value as usize (possible truncation on 32 bits).
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Checks if the address is aligned to the given size (must be a power of 2).
    #[inline(always)]
    pub const fn is_aligned(self, align: u64) -> bool {
        debug_assert!(align.is_power_of_two());
        (self.0 & (align - 1)) == 0
    }

    /// Aligns the address down to `align` bytes.
    #[inline(always)]
    pub const fn align_down(self, align: u64) -> Self {
        PhysAddr(self.0 & !(align - 1))
    }

    /// Aligns the address up to `align` bytes.
    #[inline(always)]
    pub const fn align_up(self, align: u64) -> Self {
        PhysAddr((self.0 + align - 1) & !(align - 1))
    }

    /// Returns the physical frame index (PFN = Physical Frame Number).
    #[inline(always)]
    pub const fn pfn(self) -> u64 {
        self.0 >> PAGE_SHIFT as u64
    }

    /// Aligns down to page boundary.
    #[inline(always)]
    pub const fn page_align_down(self) -> Self {
        PhysAddr(self.0 & !(PAGE_MASK as u64))
    }

    /// Aligns up to page boundary.
    #[inline(always)]
    pub const fn page_align_up(self) -> Self {
        PhysAddr((self.0 + PAGE_MASK as u64) & !(PAGE_MASK as u64))
    }

    /// Returns `true` if aligned to a page.
    #[inline(always)]
    pub const fn is_page_aligned(self) -> bool {
        (self.0 & PAGE_MASK as u64) == 0
    }

    /// Adds an offset in bytes.
    #[inline(always)]
    pub const fn add(self, offset: u64) -> Self {
        PhysAddr(self.0.wrapping_add(offset))
    }

    /// Subtracts an offset in bytes.
    #[inline(always)]
    pub const fn sub(self, offset: u64) -> Self {
        PhysAddr(self.0.wrapping_sub(offset))
    }

    /// Difference between two physical addresses (self - other).
    /// Panics (debug) if self < other.
    #[inline(always)]
    pub fn offset_from(self, base: PhysAddr) -> u64 {
        debug_assert!(self >= base, "PhysAddr::offset_from: self < base");
        self.0 - base.0
    }

    /// Null address (invalid, used as a sentinel).
    pub const NULL: PhysAddr = PhysAddr(0);

    /// Maximum representable value.
    pub const MAX: PhysAddr = PhysAddr(u64::MAX);
}

impl Add<u64> for PhysAddr {
    type Output = PhysAddr;
    #[inline(always)]
    fn add(self, rhs: u64) -> PhysAddr {
        PhysAddr(self.0.wrapping_add(rhs))
    }
}

impl AddAssign<u64> for PhysAddr {
    #[inline(always)]
    fn add_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_add(rhs);
    }
}

impl Sub<u64> for PhysAddr {
    type Output = PhysAddr;
    #[inline(always)]
    fn sub(self, rhs: u64) -> PhysAddr {
        PhysAddr(self.0.wrapping_sub(rhs))
    }
}

impl Sub<PhysAddr> for PhysAddr {
    type Output = u64;
    #[inline(always)]
    fn sub(self, rhs: PhysAddr) -> u64 {
        self.0.wrapping_sub(rhs.0)
    }
}

impl SubAssign<u64> for PhysAddr {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: u64) {
        self.0 = self.0.wrapping_sub(rhs);
    }
}

impl fmt::Debug for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PhysAddr(0x{:016x})", self.0)
    }
}

impl fmt::Display for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

impl fmt::LowerHex for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VIRTADDR — Virtual address
// ─────────────────────────────────────────────────────────────────────────────

/// Canonical 64-bit virtual address (x86_64: bits 48..63 = sign-extend bit 47).
/// Can NOT be converted to a pointer without mapping verification.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtAddr(u64);

impl VirtAddr {
    /// Creates a VirtAddr from a raw value.
    /// Canonicalizes automatically (sign-extend bit 47).
    #[inline(always)]
    pub const fn new(addr: u64) -> Self {
        // Sign-extend bit 47 to bits 48..63
        let bits = addr << 16;
        VirtAddr((bits as i64 >> 16) as u64)
    }

    /// Creates without verification.
    ///
    /// # Safety
    /// Caller guarantees that `addr` is an acceptable raw virtual value
    /// in its context. This includes non-canonical sentinels
    /// like `USER_END`, so this function performs no canonicalization.
    #[inline(always)]
    pub const unsafe fn new_unchecked(addr: u64) -> Self {
        VirtAddr(addr)
    }

    /// Creates from a raw pointer.
    ///
    /// # Safety
    /// The pointer must be a valid canonical virtual address.
    #[inline(always)]
    pub unsafe fn from_ptr<T>(ptr: *const T) -> Self {
        VirtAddr(ptr as u64)
    }

    /// Returns the raw value.
    #[inline(always)]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Returns the raw value as usize.
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }

    /// Converts to raw pointer *const T.
    ///
    /// # Safety
    /// The caller must ensure that the page is mapped,
    /// properly aligned, and that the lifetime is valid.
    #[inline(always)]
    pub const unsafe fn as_ptr<T>(self) -> *const T {
        debug_assert!(self.is_canonical());
        self.0 as *const T
    }

    /// Converts to raw pointer *mut T.
    ///
    /// # Safety
    /// Like as_ptr, plus: the page must be writable.
    #[inline(always)]
    pub const unsafe fn as_mut_ptr<T>(self) -> *mut T {
        debug_assert!(self.is_canonical());
        self.0 as *mut T
    }

    /// Checks if the address is canonical (x86_64: bits 48..63 = sign-extend 47).
    #[inline(always)]
    pub const fn is_canonical(self) -> bool {
        let high = self.0 >> 48;
        high == 0 || high == 0xFFFF
    }

    /// Checks if the address is in user space (< 0x0000_8000_0000_0000).
    #[inline(always)]
    pub const fn is_user(self) -> bool {
        self.0 < 0x0000_8000_0000_0000
    }

    /// Checks if the address is in kernel space (>= 0xFFFF_8000_0000_0000).
    #[inline(always)]
    pub const fn is_kernel(self) -> bool {
        self.0 >= 0xFFFF_8000_0000_0000
    }

    /// Aligns down to `align` bytes (power of 2).
    #[inline(always)]
    pub const fn align_down(self, align: u64) -> Self {
        VirtAddr::new(self.0 & !(align - 1))
    }

    /// Aligns up to `align` bytes (power of 2).
    #[inline(always)]
    pub const fn align_up(self, align: u64) -> Self {
        VirtAddr::new((self.0 + align - 1) & !(align - 1))
    }

    /// Aligns down to page boundary.
    #[inline(always)]
    pub const fn page_align_down(self) -> Self {
        VirtAddr::new(self.0 & !(PAGE_MASK as u64))
    }

    /// Aligns up to page boundary.
    #[inline(always)]
    pub const fn page_align_up(self) -> Self {
        VirtAddr::new((self.0 + PAGE_MASK as u64) & !(PAGE_MASK as u64))
    }

    /// Page index (virtual page number).
    #[inline(always)]
    pub const fn vpn(self) -> u64 {
        self.0 >> PAGE_SHIFT as u64
    }

    /// Extracts PML4 index (bits 39..48).
    #[inline(always)]
    pub const fn p4_index(self) -> usize {
        ((self.0 >> 39) & 0x1FF) as usize
    }

    /// Extracts PDPT index (bits 30..39).
    #[inline(always)]
    pub const fn p3_index(self) -> usize {
        ((self.0 >> 30) & 0x1FF) as usize
    }

    /// Extracts PD index (bits 21..30).
    #[inline(always)]
    pub const fn p2_index(self) -> usize {
        ((self.0 >> 21) & 0x1FF) as usize
    }

    /// Extracts PT index (bits 12..21).
    #[inline(always)]
    pub const fn p1_index(self) -> usize {
        ((self.0 >> 12) & 0x1FF) as usize
    }

    /// Extracts offset within the page (bits 0..12).
    #[inline(always)]
    pub const fn page_offset(self) -> usize {
        (self.0 & PAGE_MASK as u64) as usize
    }

    /// Extracts offset within the huge page (bits 0..21).
    #[inline(always)]
    pub const fn huge_page_offset(self) -> usize {
        (self.0 & (HUGE_PAGE_SIZE - 1) as u64) as usize
    }

    /// Difference between self and base (self - base).
    #[inline(always)]
    pub fn offset_from(self, base: VirtAddr) -> u64 {
        self.0.wrapping_sub(base.0)
    }

    pub const NULL: VirtAddr = VirtAddr(0);
    pub const KERNEL_HIGH_HALF_BASE: VirtAddr = VirtAddr(0xFFFF_8000_0000_0000);
}

impl Add<u64> for VirtAddr {
    type Output = VirtAddr;
    #[inline(always)]
    fn add(self, rhs: u64) -> VirtAddr {
        VirtAddr::new(self.0.wrapping_add(rhs))
    }
}

impl AddAssign<u64> for VirtAddr {
    #[inline(always)]
    fn add_assign(&mut self, rhs: u64) {
        *self = *self + rhs;
    }
}

impl Sub<u64> for VirtAddr {
    type Output = VirtAddr;
    #[inline(always)]
    fn sub(self, rhs: u64) -> VirtAddr {
        VirtAddr::new(self.0.wrapping_sub(rhs))
    }
}

impl Sub<VirtAddr> for VirtAddr {
    type Output = u64;
    #[inline(always)]
    fn sub(self, rhs: VirtAddr) -> u64 {
        self.0.wrapping_sub(rhs.0)
    }
}

impl SubAssign<u64> for VirtAddr {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: u64) {
        *self = *self - rhs;
    }
}

impl fmt::Debug for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VirtAddr(0x{:016x})", self.0)
    }
}

impl fmt::Display for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PAGE — Virtual page
// ─────────────────────────────────────────────────────────────────────────────

/// Identifier for a virtual page (VPN — Virtual Page Number).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Page(u64);

impl Page {
    /// Creates a Page from a VirtAddr (rounds down to page boundary).
    #[inline(always)]
    pub const fn containing(addr: VirtAddr) -> Self {
        Page(addr.as_u64() >> PAGE_SHIFT as u64)
    }

    /// Returns the raw VPN.
    #[inline(always)]
    pub const fn vpn(self) -> u64 {
        self.0
    }

    /// Returns the start virtual address of this page.
    #[inline(always)]
    pub const fn start_address(self) -> VirtAddr {
        VirtAddr::new(self.0 << PAGE_SHIFT as u64)
    }

    /// Returns the next Page.
    #[inline(always)]
    pub const fn next(self) -> Self {
        Page(self.0 + 1)
    }

    /// Returns an iterator over the range [self, end).
    #[inline(always)]
    pub fn range(self, end: Page) -> PageRange {
        PageRange { current: self, end }
    }

    /// Number of pages separating self from `end`.
    #[inline(always)]
    pub fn pages_until(self, end: Page) -> u64 {
        end.0 - self.0
    }
}

/// Iterator over a range of virtual pages.
pub struct PageRange {
    current: Page,
    end: Page,
}

impl Iterator for PageRange {
    type Item = Page;
    #[inline(always)]
    fn next(&mut self) -> Option<Page> {
        if self.current < self.end {
            let p = self.current;
            self.current = self.current.next();
            Some(p)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = (self.end.0 - self.current.0) as usize;
        (count, Some(count))
    }
}

impl ExactSizeIterator for PageRange {}

// ─────────────────────────────────────────────────────────────────────────────
// FRAME — Physical frame
// ─────────────────────────────────────────────────────────────────────────────

/// Identifier for a physical frame (PFN — Physical Frame Number).
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Frame(u64);

impl Frame {
    /// Creates a Frame from a PhysAddr (rounds down).
    #[inline(always)]
    pub const fn containing(addr: PhysAddr) -> Self {
        Frame(addr.as_u64() >> PAGE_SHIFT as u64)
    }

    /// Returns the raw PFN.
    #[inline(always)]
    pub const fn pfn(self) -> u64 {
        self.0
    }

    /// Returns the start physical address of this frame.
    #[inline(always)]
    pub const fn start_address(self) -> PhysAddr {
        PhysAddr::new(self.0 << PAGE_SHIFT as u64)
    }

    /// Returns the next Frame.
    #[inline(always)]
    pub const fn next(self) -> Self {
        Frame(self.0 + 1)
    }

    /// Iterator over the range [self, end).
    #[inline(always)]
    pub fn range(self, end: Frame) -> FrameRange {
        FrameRange { current: self, end }
    }

    /// Number of frames until `end`.
    #[inline(always)]
    pub fn frames_until(self, end: Frame) -> u64 {
        end.0 - self.0
    }

    /// Returns the start physical address of this frame.
    /// Alias for `start_address()` — provided for API consistency.
    #[inline(always)]
    pub const fn phys_addr(self) -> PhysAddr {
        self.start_address()
    }

    /// Creates a Frame from a PhysAddr (rounds down to page boundary).
    /// Alias for `Frame::containing()` — provided for API consistency.
    #[inline(always)]
    pub const fn from_phys_addr(addr: PhysAddr) -> Self {
        Frame::containing(addr)
    }

    /// Frame corresponding to PFN 0 (null frame, never allocated).
    pub const NULL: Frame = Frame(0);
}

/// Iterator over a range of physical frames.
pub struct FrameRange {
    current: Frame,
    end: Frame,
}

impl Iterator for FrameRange {
    type Item = Frame;
    #[inline(always)]
    fn next(&mut self) -> Option<Frame> {
        if self.current < self.end {
            let f = self.current;
            self.current = self.current.next();
            Some(f)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = (self.end.0 - self.current.0) as usize;
        (count, Some(count))
    }
}

impl ExactSizeIterator for FrameRange {}

// ─────────────────────────────────────────────────────────────────────────────
// PAGE FLAGS — Page protection flags / attributes
// ─────────────────────────────────────────────────────────────────────────────

/// Page flags — represent bits in the x86_64 page table entry.
/// Values match hardware bits exactly (P, RW, US, PWT, PCD,
/// A, D, PS, G, NX...) for a 1:1 hardware mapping.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct PageFlags(u64);

impl PageFlags {
    // ── x86_64 hardware bits ───────────────────────────────────────────────

    /// Bit P: page present in physical memory.
    pub const PRESENT: PageFlags = PageFlags(1 << 0);
    /// Bit RW: read/write (otherwise read-only).
    pub const WRITABLE: PageFlags = PageFlags(1 << 1);
    /// Bit US: accessible from userspace (ring 3).
    pub const USER: PageFlags = PageFlags(1 << 2);
    /// Bit PWT: write-through (otherwise write-back).
    pub const WRITE_THROUGH: PageFlags = PageFlags(1 << 3);
    /// Bit PCD: disable cache (No-Cache).
    pub const NO_CACHE: PageFlags = PageFlags(1 << 4);
    /// Bit A: page accessed (updated by CPU).
    pub const ACCESSED: PageFlags = PageFlags(1 << 5);
    /// Bit D: page modified/dirty (Dirty).
    pub const DIRTY: PageFlags = PageFlags(1 << 6);
    /// Bit PS: huge page (2MiB at PD level, 1GiB at PDPT level).
    pub const HUGE_PAGE: PageFlags = PageFlags(1 << 7);
    /// Bit G: global page (not invalidated on CR3 reload, global TLB).
    pub const GLOBAL: PageFlags = PageFlags(1 << 8);
    /// Bit NX (bit 63): No-Execute — forbids code execution.
    pub const NO_EXECUTE: PageFlags = PageFlags(1 << 63);

    // ── Software bits (bits 9..11, bits 52..62 available for the OS) ───────

    /// Software bit 9: Copy-on-Write page.
    pub const COW: PageFlags = PageFlags(1 << 9);
    /// Software bit 10: page pinned in RAM (do not swap).
    pub const PINNED: PageFlags = PageFlags(1 << 10);
    /// Software bit 11: shared memory (shared between processes).
    pub const SHARED: PageFlags = PageFlags(1 << 11);
    /// Software bit 52: page used for DMA (no CoW, no swap).
    pub const DMA: PageFlags = PageFlags(1 << 52);
    /// Software bit 53: DMA frame — no Write-Combining.
    pub const DMA_NO_WC: PageFlags = PageFlags(1 << 53);
    /// Software bit 54: 4 KiB leaf mapped via PAT write-combining entry.
    pub const WRITE_COMBINING: PageFlags = PageFlags(1 << 54);

    // ── Common combinations ────────────────────────────────────────────────

    /// Executable kernel page: P + RW + G + NX disabled.
    pub const KERNEL_CODE: PageFlags = PageFlags(Self::PRESENT.0 | Self::GLOBAL.0);

    /// R/W kernel data page, non-executable.
    pub const KERNEL_DATA: PageFlags =
        PageFlags(Self::PRESENT.0 | Self::WRITABLE.0 | Self::GLOBAL.0 | Self::NO_EXECUTE.0);

    /// R/W user page, non-executable.
    pub const USER_DATA: PageFlags =
        PageFlags(Self::PRESENT.0 | Self::WRITABLE.0 | Self::USER.0 | Self::NO_EXECUTE.0);

    /// Executable user page (code).
    pub const USER_CODE: PageFlags = PageFlags(Self::PRESENT.0 | Self::USER.0);

    /// Coherent DMA page (device memory, no-cache, R/W, global kernel).
    pub const KERNEL_DMA: PageFlags = PageFlags(
        Self::PRESENT.0
            | Self::WRITABLE.0
            | Self::GLOBAL.0
            | Self::NO_CACHE.0
            | Self::NO_EXECUTE.0
            | Self::DMA.0,
    );

    /// Empty page (not present).
    pub const EMPTY: PageFlags = PageFlags(0);

    // ── Constructor ────────────────────────────────────────────────────────

    #[inline(always)]
    pub const fn new(bits: u64) -> Self {
        PageFlags(bits)
    }

    #[inline(always)]
    pub const fn bits(self) -> u64 {
        self.0
    }

    // ── Boolean operations ─────────────────────────────────────────────────

    #[inline(always)]
    pub const fn contains(self, other: PageFlags) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline(always)]
    pub const fn intersects(self, other: PageFlags) -> bool {
        (self.0 & other.0) != 0
    }

    #[inline(always)]
    pub const fn set(self, flag: PageFlags) -> Self {
        PageFlags(self.0 | flag.0)
    }

    #[inline(always)]
    pub const fn clear(self, flag: PageFlags) -> Self {
        PageFlags(self.0 & !flag.0)
    }

    #[inline(always)]
    pub const fn toggle(self, flag: PageFlags) -> Self {
        PageFlags(self.0 ^ flag.0)
    }

    /// Checks if the page is present.
    #[inline(always)]
    pub const fn is_present(self) -> bool {
        self.contains(Self::PRESENT)
    }

    /// Checks if the page is writable.
    #[inline(always)]
    pub const fn is_writable(self) -> bool {
        self.contains(Self::WRITABLE)
    }

    /// Checks if the page is accessible from userspace.
    #[inline(always)]
    pub const fn is_user(self) -> bool {
        self.contains(Self::USER)
    }

    /// Checks if the page is executable (NX not set).
    #[inline(always)]
    pub const fn is_executable(self) -> bool {
        !self.contains(Self::NO_EXECUTE)
    }

    /// Checks if this is a huge page.
    #[inline(always)]
    pub const fn is_huge(self) -> bool {
        self.contains(Self::HUGE_PAGE)
    }

    /// Checks if the page is CoW.
    #[inline(always)]
    pub const fn is_cow(self) -> bool {
        self.contains(Self::COW)
    }

    /// Checks if the page is pinned in RAM.
    #[inline(always)]
    pub const fn is_pinned(self) -> bool {
        self.contains(Self::PINNED)
    }
}

impl BitAnd for PageFlags {
    type Output = Self;
    #[inline(always)]
    fn bitand(self, rhs: Self) -> Self {
        PageFlags(self.0 & rhs.0)
    }
}

impl BitOr for PageFlags {
    type Output = Self;
    #[inline(always)]
    fn bitor(self, rhs: Self) -> Self {
        PageFlags(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for PageFlags {
    #[inline(always)]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAndAssign for PageFlags {
    #[inline(always)]
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Not for PageFlags {
    type Output = Self;
    #[inline(always)]
    fn not(self) -> Self {
        PageFlags(!self.0)
    }
}

impl fmt::Debug for PageFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PageFlags(")?;
        let mut first = true;
        let flags = [
            (Self::PRESENT, "PRESENT"),
            (Self::WRITABLE, "WRITABLE"),
            (Self::USER, "USER"),
            (Self::WRITE_THROUGH, "WRITE_THROUGH"),
            (Self::NO_CACHE, "NO_CACHE"),
            (Self::ACCESSED, "ACCESSED"),
            (Self::DIRTY, "DIRTY"),
            (Self::HUGE_PAGE, "HUGE"),
            (Self::GLOBAL, "GLOBAL"),
            (Self::NO_EXECUTE, "NX"),
            (Self::COW, "COW"),
            (Self::PINNED, "PINNED"),
            (Self::SHARED, "SHARED"),
            (Self::DMA, "DMA"),
            (Self::DMA_NO_WC, "DMA_NO_WC"),
            (Self::WRITE_COMBINING, "WC"),
        ];
        for (flag, name) in &flags {
            if self.contains(*flag) {
                if !first {
                    write!(f, "|")?;
                }
                write!(f, "{}", name)?;
                first = false;
            }
        }
        if first {
            write!(f, "EMPTY")?;
        }
        write!(f, ")")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ZONE TYPE — Memory zone category
// ─────────────────────────────────────────────────────────────────────────────

/// Category of physical memory zone.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u8)]
pub enum ZoneType {
    /// DMA zone — <16 MiB, for legacy 32-bit devices.
    Dma = 0,
    /// DMA32 zone — <4 GiB, for 32-bit PCIe devices.
    Dma32 = 1,
    /// NORMAL zone — Main RAM (>= 4 GiB on 64 bits, >= 896 MiB on 32 bits).
    Normal = 2,
    /// HIGH zone — High memory (32 bits only, >896 MiB).
    High = 3,
    /// MOVABLE zone — Movable pages to defragment huge pages.
    Movable = 4,
}

impl ZoneType {
    /// Returns the total number of known zones.
    pub const COUNT: usize = 5;

    /// Returns the numerical index of the zone.
    #[inline(always)]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Converts an index to ZoneType (None if invalid).
    #[inline(always)]
    pub const fn from_index(idx: usize) -> Option<ZoneType> {
        match idx {
            0 => Some(ZoneType::Dma),
            1 => Some(ZoneType::Dma32),
            2 => Some(ZoneType::Normal),
            3 => Some(ZoneType::High),
            4 => Some(ZoneType::Movable),
            _ => None,
        }
    }

    /// Determines the appropriate zone for a given physical address.
    #[inline]
    pub fn for_phys_addr(addr: PhysAddr) -> ZoneType {
        use super::constants::{ZONE_DMA32_END, ZONE_DMA_END};
        let a = addr.as_usize();
        if a < ZONE_DMA_END {
            ZoneType::Dma
        } else if a < ZONE_DMA32_END {
            ZoneType::Dma32
        } else {
            ZoneType::Normal
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ALLOC FLAGS — Allocation options
// ─────────────────────────────────────────────────────────────────────────────

/// Flags controlling physical allocator behavior.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct AllocFlags(u32);

impl AllocFlags {
    /// No special flags.
    pub const NONE: AllocFlags = AllocFlags(0);
    /// Do not block if memory is exhausted — returns Err immediately.
    pub const NO_WAIT: AllocFlags = AllocFlags(1 << 0);
    /// Allocation for DMA (DMA zone < 16 MiB required).
    pub const DMA: AllocFlags = AllocFlags(1 << 1);
    /// DMA32 allocation (DMA32 zone < 4 GiB).
    pub const DMA32: AllocFlags = AllocFlags(1 << 2);
    /// Zero initialize pages before returning them.
    pub const ZEROED: AllocFlags = AllocFlags(1 << 3);
    /// Urgent allocation (use emergency pool if needed).
    pub const EMERGENCY: AllocFlags = AllocFlags(1 << 4);
    /// Execute from an atomic context (no blocking allowed).
    pub const ATOMIC: AllocFlags = AllocFlags(Self::NO_WAIT.0 | 1 << 5);
    /// Guard page (never map into global TLB cache).
    pub const GUARD: AllocFlags = AllocFlags(1 << 6);
    /// MOVABLE allocation (eligible for inter-node migration).
    pub const MOVABLE: AllocFlags = AllocFlags(1 << 7);
    /// Pin page in RAM (do not allow swap).
    pub const PIN: AllocFlags = AllocFlags(1 << 8);

    #[inline(always)]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[inline(always)]
    pub const fn contains(self, f: AllocFlags) -> bool {
        (self.0 & f.0) == f.0
    }

    #[inline(always)]
    pub const fn set(self, f: AllocFlags) -> Self {
        AllocFlags(self.0 | f.0)
    }

    /// Zone required by these flags.
    #[inline]
    pub const fn required_zone(self) -> ZoneType {
        if self.contains(Self::DMA) {
            ZoneType::Dma
        } else if self.contains(Self::DMA32) {
            ZoneType::Dma32
        } else if self.contains(Self::MOVABLE) {
            ZoneType::Movable
        } else {
            ZoneType::Normal
        }
    }
}

impl BitOr for AllocFlags {
    type Output = Self;
    #[inline(always)]
    fn bitor(self, rhs: Self) -> Self {
        AllocFlags(self.0 | rhs.0)
    }
}

impl fmt::Debug for AllocFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AllocFlags(0b{:016b})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ALLOC ERROR — Allocation errors
// ─────────────────────────────────────────────────────────────────────────────

/// Errors that can occur during memory allocation.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AllocError {
    /// No free memory left in the requested zone.
    OutOfMemory,
    /// Fragmentation: no contiguous block large enough.
    Fragmentation,
    /// Invalid parameters (zero order, order too large, invalid align).
    InvalidParams,
    /// The allocator is not yet initialized.
    NotInitialized,
    /// Allocation would block but NO_WAIT was set.
    WouldBlock,
    /// Requested memory zone does not exist or is not initialized.
    ZoneUnavailable,
    /// Exceeded memory limit for this process (cgroup).
    LimitExceeded,
}

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllocError::OutOfMemory => write!(f, "Out of memory"),
            AllocError::Fragmentation => write!(f, "Memory fragmentation"),
            AllocError::InvalidParams => write!(f, "Invalid allocation parameters"),
            AllocError::NotInitialized => write!(f, "Allocator not initialized"),
            AllocError::WouldBlock => write!(f, "Allocation would block (NO_WAIT set)"),
            AllocError::ZoneUnavailable => write!(f, "Memory zone unavailable"),
            AllocError::LimitExceeded => write!(f, "Memory limit exceeded"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// STATIC SIZE CHECKS
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(
    core::mem::size_of::<PhysAddr>() == 8,
    "PhysAddr doit faire 8 bytes"
);
const _: () = assert!(
    core::mem::size_of::<VirtAddr>() == 8,
    "VirtAddr doit faire 8 bytes"
);
const _: () = assert!(core::mem::size_of::<Page>() == 8, "Page doit faire 8 bytes");
const _: () = assert!(
    core::mem::size_of::<Frame>() == 8,
    "Frame doit faire 8 bytes"
);
const _: () = assert!(
    core::mem::size_of::<PageFlags>() == 8,
    "PageFlags doit faire 8 bytes"
);
const _: () = assert!(
    core::mem::size_of::<AllocFlags>() == 4,
    "AllocFlags doit faire 4 bytes"
);
const _: () = assert!(
    core::mem::size_of::<ZoneType>() == 1,
    "ZoneType doit faire 1 byte"
);
