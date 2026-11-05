// kernel/src/memory/virtual/vma/descriptor.rs
//
// VMA (Virtual Memory Area) descriptor.
// Represents a region of a process's virtual address space.
// Layer 0 — no external dependency except `spin`.

use crate::memory::core::{PageFlags, VirtAddr, VirtRange};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// VMA FLAGS
// ─────────────────────────────────────────────────────────────────────────────

/// Flags describing the type and properties of a VMA.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct VmaFlags(u32);

impl VmaFlags {
    pub const NONE: VmaFlags = VmaFlags(0);
    pub const READ: VmaFlags = VmaFlags(1 << 0);
    pub const WRITE: VmaFlags = VmaFlags(1 << 1);
    pub const EXEC: VmaFlags = VmaFlags(1 << 2);
    pub const SHARED: VmaFlags = VmaFlags(1 << 3);
    pub const ANONYMOUS: VmaFlags = VmaFlags(1 << 4);
    pub const STACK: VmaFlags = VmaFlags(1 << 5);
    pub const HEAP: VmaFlags = VmaFlags(1 << 6);
    pub const FIXED: VmaFlags = VmaFlags(1 << 7); // Fixed address (MAP_FIXED)
    pub const GROWSDOWN: VmaFlags = VmaFlags(1 << 8); // Stack that grows downward
    pub const COW: VmaFlags = VmaFlags(1 << 9); // Active Copy-on-Write
    pub const LOCKED: VmaFlags = VmaFlags(1 << 10); // mlock
    pub const HUGETLB: VmaFlags = VmaFlags(1 << 11); // Huge pages TLB
    pub const IO: VmaFlags = VmaFlags(1 << 12); // I/O mapping (non-cacheable)
    pub const KERNEL: VmaFlags = VmaFlags(1 << 13); // Kernel VMA
    pub const DONTEXPAND: VmaFlags = VmaFlags(1 << 14); // Non-expandable
    pub const WIPEONFORK: VmaFlags = VmaFlags(1 << 15); // Clear on fork
    pub const DONTCOPY: VmaFlags = VmaFlags(1 << 16); // Not copied on fork (SignalTcb)

    pub const fn contains(self, other: VmaFlags) -> bool {
        (self.0 & other.0) == other.0
    }
    pub const fn bits(self) -> u32 {
        self.0
    }
    /// Constructs VmaFlags from a raw value.
    pub const fn from_bits(v: u32) -> Self {
        VmaFlags(v)
    }
}

impl core::ops::BitOr for VmaFlags {
    type Output = VmaFlags;
    fn bitor(self, other: VmaFlags) -> VmaFlags {
        VmaFlags(self.0 | other.0)
    }
}

impl core::ops::BitOrAssign for VmaFlags {
    fn bitor_assign(&mut self, other: VmaFlags) {
        self.0 |= other.0;
    }
}

impl core::fmt::Debug for VmaFlags {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "VmaFlags({:#010x})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BACKING TYPE (page source)
// ─────────────────────────────────────────────────────────────────────────────

/// Source of pages for a VMA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VmaBacking {
    /// Anonymous pages (zero-filled on demand).
    Anonymous = 0,
    /// Memory-mapped file (file-backed).
    File = 1,
    /// Device memory mapping.
    Device = 2,
    /// Directly mapped kernel pages (no demand paging).
    Direct = 3,
    /// Shared memory segment (IPC shared memory).
    Shared = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// VMA DESCRIPTOR
// ─────────────────────────────────────────────────────────────────────────────

/// Full descriptor of a VMA.
/// This structure is the single source of truth for a region's properties.
///
/// Allocated in the slab allocator (< 2 KiB).
#[repr(C, align(64))]
pub struct VmaDescriptor {
    // ── Virtual limits ──────────────────────────────────────────────────────
    /// Region start (inclusive, aligned to PAGE_SIZE).
    pub start: VirtAddr,
    /// Region end (exclusive, aligned to PAGE_SIZE).
    pub end: VirtAddr,

    // ── Properties ──────────────────────────────────────────────────────────
    pub flags: VmaFlags,
    pub page_flags: PageFlags,
    pub backing: VmaBacking,

    // ── Backing file (if VmaBacking::File) ───────────────────────────────────
    /// Inode ID (0 = no file).
    pub inode_id: u64,
    /// Offset within the file.
    pub file_offset: u64,

    // ── Statistics ──────────────────────────────────────────────────────────
    pub pages_resident: AtomicU32,
    pub pages_swapped: AtomicU32,
    pub page_faults: AtomicU64,
    pub cow_breaks: AtomicU32,

    // ── AVL tree (internal links) ────────────────────────────────────────────
    /// Next node (ascending address order).
    pub(crate) rb_left: *mut VmaDescriptor,
    pub(crate) rb_right: *mut VmaDescriptor,
    pub(crate) rb_height: i32,

    pub _pad: [u8; 4],
}

// SAFETY: VmaDescriptor is a RON (Record Of Non-moveable), protected by
//         the parent address space lock.
unsafe impl Send for VmaDescriptor {}
unsafe impl Sync for VmaDescriptor {}

impl VmaDescriptor {
    /// Creates a VMA descriptor with basic parameters.
    pub const fn new(
        start: VirtAddr,
        end: VirtAddr,
        flags: VmaFlags,
        page_flags: PageFlags,
        backing: VmaBacking,
    ) -> Self {
        VmaDescriptor {
            start,
            end,
            flags,
            page_flags,
            backing,
            inode_id: 0,
            file_offset: 0,
            pages_resident: AtomicU32::new(0),
            pages_swapped: AtomicU32::new(0),
            page_faults: AtomicU64::new(0),
            cow_breaks: AtomicU32::new(0),
            rb_left: core::ptr::null_mut(),
            rb_right: core::ptr::null_mut(),
            rb_height: 1,
            _pad: [0; 4],
        }
    }

    /// Returns the size in bytes of the VMA.
    #[inline]
    pub fn size(&self) -> usize {
        (self.end.as_u64() - self.start.as_u64()) as usize
    }

    /// Returns the number of pages in the VMA.
    #[inline]
    pub fn n_pages(&self) -> usize {
        self.size() / crate::memory::core::PAGE_SIZE
    }

    /// Checks whether the virtual address falls within this VMA.
    #[inline]
    pub fn contains(&self, addr: VirtAddr) -> bool {
        addr.as_u64() >= self.start.as_u64() && addr.as_u64() < self.end.as_u64()
    }

    /// Returns the virtual range of this VMA.
    #[inline]
    pub fn range(&self) -> VirtRange {
        VirtRange::from_range(self.start, self.end)
    }

    /// Increments the page fault counter.
    #[inline]
    pub fn record_fault(&self) {
        self.page_faults.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the CoW break counter.
    #[inline]
    pub fn record_cow_break(&self) {
        self.cow_breaks.fetch_add(1, Ordering::Relaxed);
    }

    /// Checks whether this VMA can be merged with `other` (adjacency + identical flags).
    pub fn can_merge_with(&self, other: &VmaDescriptor) -> bool {
        self.end == other.start
            && self.flags == other.flags
            && self.page_flags == other.page_flags
            && self.backing == other.backing
            && self.inode_id == other.inode_id
    }

    /// Clone only the VMA metadata, resetting intrusive tree links.
    pub fn clone_metadata(&self) -> Self {
        let mut cloned = VmaDescriptor::new(
            self.start,
            self.end,
            self.flags,
            self.page_flags,
            self.backing,
        );
        cloned.inode_id = self.inode_id;
        cloned.file_offset = self.file_offset;
        cloned.pages_resident.store(
            self.pages_resident.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        cloned.pages_swapped.store(
            self.pages_swapped.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        cloned
    }
}

impl core::fmt::Debug for VmaDescriptor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "VMA[{:#x}..{:#x} flags={:?} backing={:?}]",
            self.start.as_u64(),
            self.end.as_u64(),
            self.flags,
            self.backing
        )
    }
}
