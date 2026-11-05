// kernel/src/memory/core/address.rs
//
// Address translations, alignment assertions, utilities.
// Layer 0 — no external dependencies.

use super::constants::{HUGE_PAGE_SHIFT, HUGE_PAGE_SIZE, PAGE_SHIFT, PAGE_SIZE};
use super::layout::{
    KERNEL_HEAP_SIZE, KERNEL_HEAP_START, KERNEL_PHYS_OFFSET, PHYS_MAP_BASE, PHYS_MAP_SIZE,
};
use super::types::{Frame, Page, PhysAddr, VirtAddr};

// ─────────────────────────────────────────────────────────────────────────────
// PHYSICAL ↔ VIRTUAL TRANSLATIONS (via the kernel physmap region)
// ─────────────────────────────────────────────────────────────────────────────

/// Translates a physical address to a virtual address via direct physmap.
///
/// The physmap maps the entirety of physical RAM to PHYS_MAP_BASE as
/// read/write (kernel mode only). Always valid during kernel execution
/// after init_phys_map().
///
/// # Panics
/// In debug mode, panics if the address exceeds PHYS_MAP_SIZE.
#[inline(always)]
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    debug_assert!(
        phys.as_u64() < PHYS_MAP_SIZE as u64,
        "phys_to_virt: adresse physique 0x{:016x} dépasse la physmap (max 0x{:016x})",
        phys.as_u64(),
        PHYS_MAP_SIZE
    );
    // SAFETY: physmap initialized at boot; offset in the canonical kernel space.
    unsafe { VirtAddr::new_unchecked(PHYS_MAP_BASE.as_u64() + phys.as_u64()) }
}

/// Translates a physmap virtual address to a physical address.
///
/// # Panics
/// In debug mode, panics if the address is outside the physmap.
#[inline(always)]
pub fn virt_to_phys_physmap(virt: VirtAddr) -> PhysAddr {
    debug_assert!(
        virt >= PHYS_MAP_BASE && virt.as_u64() < PHYS_MAP_BASE.as_u64() + PHYS_MAP_SIZE as u64,
        "virt_to_phys_physmap: adresse 0x{:016x} hors physmap",
        virt.as_u64()
    );
    PhysAddr::new(virt.as_u64() - PHYS_MAP_BASE.as_u64())
}

/// Translates a kernel virtual address (.text/.data/.bss section) to physical.
/// Only applicable for addresses in the direct kernel mapping.
///
/// # Panics
/// In debug mode, panics if the address precedes KERNEL_PHYS_OFFSET.
#[inline(always)]
pub fn kernel_virt_to_phys(virt: VirtAddr) -> PhysAddr {
    debug_assert!(
        virt.as_u64() >= KERNEL_PHYS_OFFSET.as_u64(),
        "kernel_virt_to_phys: adresse 0x{:016x} avant KERNEL_PHYS_OFFSET",
        virt.as_u64()
    );
    PhysAddr::new(virt.as_u64() - KERNEL_PHYS_OFFSET.as_u64())
}

/// Translates a kernel physical address to a .text/.data virtual address.
#[inline(always)]
pub fn kernel_phys_to_virt(phys: PhysAddr) -> VirtAddr {
    // SAFETY: Arithmetic on the canonical address space.
    unsafe { VirtAddr::new_unchecked(phys.as_u64() + KERNEL_PHYS_OFFSET.as_u64()) }
}

// ─────────────────────────────────────────────────────────────────────────────
// ALIGNMENT CHECKS
// ─────────────────────────────────────────────────────────────────────────────

/// Checks that `size` is non-zero and a power of 2.
#[inline(always)]
pub const fn is_power_of_two(size: usize) -> bool {
    size != 0 && (size & (size - 1)) == 0
}

/// Rounds `value` down to a multiple of `align`.
/// `align` must be a power of 2.
#[inline(always)]
pub const fn align_down(value: usize, align: usize) -> usize {
    debug_assert!(is_power_of_two(align));
    value & !(align - 1)
}

/// Rounds `value` up to a multiple of `align`.
/// `align` must be a power of 2.
#[inline(always)]
pub const fn align_up(value: usize, align: usize) -> usize {
    debug_assert!(is_power_of_two(align));
    (value + align - 1) & !(align - 1)
}

/// Checks if `value` is aligned to `align`.
#[inline(always)]
pub const fn is_aligned(value: usize, align: usize) -> bool {
    (value & (align - 1)) == 0
}

/// Rounds down to page boundary.
#[inline(always)]
pub const fn page_align_down(value: usize) -> usize {
    align_down(value, PAGE_SIZE)
}

/// Rounds up to page boundary.
#[inline(always)]
pub const fn page_align_up(value: usize) -> usize {
    align_up(value, PAGE_SIZE)
}

/// Number of pages required to cover `size` bytes.
#[inline(always)]
pub const fn pages_for(size: usize) -> usize {
    (size + PAGE_SIZE - 1) >> PAGE_SHIFT
}

/// Number of huge pages (2 MiB) required to cover `size` bytes.
#[inline(always)]
pub const fn huge_pages_for(size: usize) -> usize {
    (size + HUGE_PAGE_SIZE - 1) >> HUGE_PAGE_SHIFT
}

/// Bytes → pages (rounded up).
#[inline(always)]
pub const fn bytes_to_pages(bytes: usize) -> usize {
    pages_for(bytes)
}

/// Pages → bytes.
#[inline(always)]
pub const fn pages_to_bytes(pages: usize) -> usize {
    pages << PAGE_SHIFT
}

// ─────────────────────────────────────────────────────────────────────────────
// CALCULATIONS ON FRAMES / PAGES
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the Frame containing physical address `addr`.
#[inline(always)]
pub const fn frame_containing(addr: PhysAddr) -> Frame {
    Frame::containing(addr)
}

/// Returns the Page containing virtual address `addr`.
#[inline(always)]
pub const fn page_containing(addr: VirtAddr) -> Page {
    Page::containing(addr)
}

/// Returns the physical address of a PFN.
#[inline(always)]
pub const fn pfn_to_phys(pfn: u64) -> PhysAddr {
    PhysAddr::new(pfn << PAGE_SHIFT as u64)
}

/// Returns the PFN of a physical address (rounded down).
#[inline(always)]
pub const fn phys_to_pfn(addr: PhysAddr) -> u64 {
    addr.as_u64() >> PAGE_SHIFT as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// x86_64 CANONICALIZATION
// ─────────────────────────────────────────────────────────────────────────────

/// Canonicalizes a 64-bit virtual address (sign-extension bit 47).
/// Required before any loading into CR3 or page table access.
#[inline(always)]
pub const fn canonicalize(addr: u64) -> u64 {
    // Shift left 16, then arithmetic shift right 16 = sign-extend bit 47
    (((addr << 16) as i64) >> 16) as u64
}

/// Checks if an address is canonical according to x86_64 rules.
/// Non-canonical addresses trigger a #GP fault.
#[inline(always)]
pub const fn is_canonical(addr: u64) -> bool {
    let extended = canonicalize(addr);
    extended == addr
}

/// Checks if a pointer is usable as a canonical kernel address.
#[inline(always)]
pub const fn is_kernel_canonical(addr: u64) -> bool {
    is_canonical(addr) && addr >= 0xFFFF_8000_0000_0000
}

/// Checks if a pointer is usable as a canonical user address.
#[inline(always)]
pub const fn is_user_canonical(addr: u64) -> bool {
    is_canonical(addr) && addr < 0x0000_8000_0000_0000
}

// ─────────────────────────────────────────────────────────────────────────────
// ADDRESS RANGES
// ─────────────────────────────────────────────────────────────────────────────

/// Physical address range [start, end).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct PhysRange {
    pub start: PhysAddr,
    pub end: PhysAddr,
}

impl PhysRange {
    /// Creates a range [start, start + size).
    #[inline(always)]
    pub const fn new(start: PhysAddr, size: usize) -> Self {
        PhysRange {
            start,
            end: PhysAddr::new(start.as_u64() + size as u64),
        }
    }

    /// Creates a range [start, end).
    #[inline(always)]
    pub const fn from_range(start: PhysAddr, end: PhysAddr) -> Self {
        PhysRange { start, end }
    }

    /// Size in bytes.
    #[inline(always)]
    pub const fn size(self) -> usize {
        (self.end.as_u64() - self.start.as_u64()) as usize
    }

    /// Number of pages in the range (rounded up).
    #[inline(always)]
    pub fn page_count(self) -> usize {
        pages_for(self.size())
    }

    /// Checks if the range contains `addr`.
    #[inline(always)]
    pub const fn contains(self, addr: PhysAddr) -> bool {
        addr.as_u64() >= self.start.as_u64() && addr.as_u64() < self.end.as_u64()
    }

    /// Checks if the range is empty.
    #[inline(always)]
    pub const fn is_empty(self) -> bool {
        self.start.as_u64() >= self.end.as_u64()
    }

    /// Checks if two ranges overlap.
    #[inline(always)]
    pub const fn overlaps(self, other: PhysRange) -> bool {
        self.start.as_u64() < other.end.as_u64() && other.start.as_u64() < self.end.as_u64()
    }

    /// Alignment of start downwards to a page.
    #[inline(always)]
    pub fn page_aligned(self) -> PhysRange {
        PhysRange {
            start: self.start.page_align_down(),
            end: self.end.page_align_up(),
        }
    }
}

/// Virtual address range [start, end).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct VirtRange {
    pub start: VirtAddr,
    pub end: VirtAddr,
}

impl VirtRange {
    /// Creates a range [start, start + size).
    #[inline(always)]
    pub const fn new(start: VirtAddr, size: usize) -> Self {
        VirtRange {
            start,
            end: unsafe { VirtAddr::new_unchecked(start.as_u64() + size as u64) },
        }
    }

    /// Creates a range [start, end).
    #[inline(always)]
    pub const fn from_range(start: VirtAddr, end: VirtAddr) -> Self {
        VirtRange { start, end }
    }

    /// Size in bytes.
    #[inline(always)]
    pub const fn size(self) -> usize {
        (self.end.as_u64() - self.start.as_u64()) as usize
    }

    /// Number of pages in the range.
    #[inline(always)]
    pub fn page_count(self) -> usize {
        pages_for(self.size())
    }

    /// Checks if the range contains `addr`.
    #[inline(always)]
    pub const fn contains(self, addr: VirtAddr) -> bool {
        addr.as_u64() >= self.start.as_u64() && addr.as_u64() < self.end.as_u64()
    }

    /// Checks if the two ranges overlap.
    #[inline(always)]
    pub const fn overlaps(self, other: VirtRange) -> bool {
        self.start.as_u64() < other.end.as_u64() && other.start.as_u64() < self.end.as_u64()
    }

    /// Aligns boundaries to pages.
    #[inline(always)]
    pub fn page_aligned(self) -> VirtRange {
        VirtRange {
            start: self.start.page_align_down(),
            end: self.end.page_align_up(),
        }
    }

    /// Checks if the range is entirely in kernel space.
    #[inline(always)]
    pub const fn is_kernel(self) -> bool {
        self.start.is_kernel() && self.end.is_kernel()
    }

    /// Checks if the range is entirely in user space.
    #[inline(always)]
    pub const fn is_user(self) -> bool {
        self.start.is_user() && self.end.is_user()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// KERNEL HEAP HELPERS
// ─────────────────────────────────────────────────────────────────────────────

/// Checks if a virtual address belongs to the kernel heap.
#[inline(always)]
pub fn in_kernel_heap(addr: VirtAddr) -> bool {
    addr >= KERNEL_HEAP_START
        && addr.as_u64() < KERNEL_HEAP_START.as_u64() + KERNEL_HEAP_SIZE as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// INLINE TESTS (verifiable in debug mode)
// ─────────────────────────────────────────────────────────────────────────────

/// Executes consistency assertions for the address module at boot.
pub fn assert_invariants() {
    // PAGE_SIZE / PAGE_SHIFT consistency
    debug_assert_eq!(1usize << PAGE_SHIFT, PAGE_SIZE);
    // HUGE_PAGE_SIZE / HUGE_PAGE_SHIFT consistency
    debug_assert_eq!(1usize << HUGE_PAGE_SHIFT, HUGE_PAGE_SIZE);
    // The physmap starts in kernel space
    debug_assert!(PHYS_MAP_BASE.is_kernel());
    // align_down(0, PAGE_SIZE) == 0
    debug_assert_eq!(page_align_down(0), 0);
    // align_up(1, PAGE_SIZE) == PAGE_SIZE
    debug_assert_eq!(page_align_up(1), PAGE_SIZE);
    // align_up(PAGE_SIZE, PAGE_SIZE) == PAGE_SIZE
    debug_assert_eq!(page_align_up(PAGE_SIZE), PAGE_SIZE);
    // pages_for(0) == 0
    debug_assert_eq!(pages_for(0), 0);
    // pages_for(1) == 1
    debug_assert_eq!(pages_for(1), 1);
    // pages_for(PAGE_SIZE) == 1
    debug_assert_eq!(pages_for(PAGE_SIZE), 1);
    // pages_for(PAGE_SIZE + 1) == 2
    debug_assert_eq!(pages_for(PAGE_SIZE + 1), 2);
    // canonicalize preserves canonical addresses
    debug_assert_eq!(canonicalize(0xFFFF_8000_0000_0000), 0xFFFF_8000_0000_0000);
    debug_assert_eq!(canonicalize(0x0000_7FFF_FFFF_FFFF), 0x0000_7FFF_FFFF_FFFF);
    // A non-canonical address must be detected
    debug_assert!(!is_canonical(0x0001_0000_0000_0000));
}

// ─────────────────────────────────────────────────────────────────────────────
// STATIC CHECKS
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(
    is_power_of_two(PAGE_SIZE),
    "PAGE_SIZE doit être puissance de 2"
);
const _: () = assert!(
    is_power_of_two(HUGE_PAGE_SIZE),
    "HUGE_PAGE_SIZE doit être puissance de 2"
);
const _: () = assert!(PAGE_SIZE == 4096, "PAGE_SIZE doit être 4096 pour x86_64");
const _: () = assert!(HUGE_PAGE_SIZE == 2097152, "HUGE_PAGE_SIZE doit être 2MiB");
const _: () = assert!(
    align_up(0, PAGE_SIZE) == 0,
    "align_up(0, PAGE_SIZE) doit être 0"
);
const _: () = assert!(
    align_up(1, PAGE_SIZE) == PAGE_SIZE,
    "align_up(1, PAGE_SIZE) doit être PAGE_SIZE"
);
const _: () = assert!(
    align_down(PAGE_SIZE + 1, PAGE_SIZE) == PAGE_SIZE,
    "align_down doit arrondir vers le bas"
);
