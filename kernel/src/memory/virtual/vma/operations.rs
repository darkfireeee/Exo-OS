// kernel/src/memory/virtual/vma/operations.rs
//
// Operations on VMAs: mmap, munmap, mprotect, split, merge.
// Layer 0 — no external dependency except `spin`.

use super::descriptor::{VmaBacking, VmaDescriptor, VmaFlags};
use super::tree::VmaTree;
use crate::memory::core::{AllocError, PageFlags, VirtAddr, PAGE_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// VMA ALLOCATION PARAMETERS
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters to create a new VMA.
#[derive(Debug, Clone)]
pub struct VmaAllocParams {
    pub hint_addr: Option<VirtAddr>, // Desired address (can be ignored)
    pub size: usize,
    pub flags: VmaFlags,
    pub page_flags: PageFlags,
    pub backing: VmaBacking,
    pub inode_id: u64,
    pub file_offset: u64,
    pub fixed: bool, // MAP_FIXED: use hint_addr exactly
}

impl VmaAllocParams {
    pub fn anonymous(size: usize, flags: VmaFlags, page_flags: PageFlags) -> Self {
        VmaAllocParams {
            hint_addr: None,
            size,
            flags: flags | VmaFlags::ANONYMOUS,
            page_flags,
            backing: VmaBacking::Anonymous,
            inode_id: 0,
            file_offset: 0,
            fixed: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GAP MANAGEMENT (free ranges)
// ─────────────────────────────────────────────────────────────────────────────

/// Searches for an available gap in the tree for a VMA of `size` bytes.
///
/// Strategy: first-fit starting from `hint_start`, then from `min_addr`
/// if not found.
pub fn find_gap(
    tree: &VmaTree,
    size: usize,
    hint_start: VirtAddr,
    min_addr: VirtAddr,
    max_addr: VirtAddr,
) -> Option<VirtAddr> {
    let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Clamp hint
    let start_hint =
        if hint_start.as_u64() >= min_addr.as_u64() && hint_start.as_u64() < max_addr.as_u64() {
            hint_start
        } else {
            min_addr
        };

    // Try starting from hint
    if let Some(addr) = scan_gap(tree, start_hint, max_addr, aligned_size) {
        return Some(addr);
    }
    // Try from min_addr
    if start_hint.as_u64() > min_addr.as_u64() {
        return scan_gap(tree, min_addr, start_hint, aligned_size);
    }
    None
}

fn scan_gap(tree: &VmaTree, from: VirtAddr, to: VirtAddr, size: usize) -> Option<VirtAddr> {
    let mut cursor = align_up_addr(from, PAGE_SIZE);
    for vma in tree.iter() {
        // Gap between cursor and start of this VMA
        if vma.start.as_u64() > cursor.as_u64() {
            let gap = (vma.start.as_u64() - cursor.as_u64()) as usize;
            if gap >= size && cursor.as_u64() + size as u64 <= to.as_u64() {
                return Some(cursor);
            }
        }
        // Advance past end of this VMA
        if vma.end.as_u64() > cursor.as_u64() {
            cursor = VirtAddr::new(vma.end.as_u64());
        }
    }
    // Gap after all VMAs
    if cursor.as_u64() + size as u64 <= to.as_u64() {
        return Some(cursor);
    }
    None
}

fn align_up_addr(addr: VirtAddr, align: usize) -> VirtAddr {
    VirtAddr::new((addr.as_u64() + align as u64 - 1) & !(align as u64 - 1))
}

// ─────────────────────────────────────────────────────────────────────────────
// VMA SPLIT
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a VMA split.
pub struct SplitResult {
    /// Left part (start..split_point).
    pub left: VmaDescriptor,
    /// Right part (split_point..end).
    pub right: VmaDescriptor,
}

/// Splits `vma` in two at address `split_point`.
///
/// `split_point` must be aligned to PAGE_SIZE and within [start..end).
pub fn split_vma(vma: &VmaDescriptor, split_point: VirtAddr) -> Result<SplitResult, AllocError> {
    let sp = split_point.as_u64();
    if sp <= vma.start.as_u64() || sp >= vma.end.as_u64() {
        return Err(AllocError::InvalidParams);
    }
    if !sp.is_multiple_of(PAGE_SIZE as u64) {
        return Err(AllocError::InvalidParams);
    }

    let mut left = VmaDescriptor::new(
        vma.start,
        split_point,
        vma.flags,
        vma.page_flags,
        vma.backing,
    );
    // Propagate file association: left starts from vma.start, so
    // its file_offset is identical to that of the original VMA.
    left.inode_id = vma.inode_id;
    left.file_offset = vma.file_offset;

    let mut right =
        VmaDescriptor::new(split_point, vma.end, vma.flags, vma.page_flags, vma.backing);
    right.inode_id = vma.inode_id;
    right.file_offset = vma.file_offset + (sp - vma.start.as_u64());

    Ok(SplitResult { left, right })
}

// ─────────────────────────────────────────────────────────────────────────────
// MPROTECT
// ─────────────────────────────────────────────────────────────────────────────

/// Result of mprotect: VMAs to insert after transformation.
pub struct MprotectResult {
    /// VMA before the modified range (can be None if start == vma.start).
    pub before: Option<VmaDescriptor>,
    /// Modified VMA (the range [start..end) with new flags).
    pub middle: VmaDescriptor,
    /// VMA after the modified range (can be None if end == vma.end).
    pub after: Option<VmaDescriptor>,
}

/// Applies a flags modification on the range [start..end) of a VMA.
pub fn mprotect_vma(
    vma: &VmaDescriptor,
    start: VirtAddr,
    end: VirtAddr,
    new_flags: PageFlags,
) -> Result<MprotectResult, AllocError> {
    if start.as_u64() < vma.start.as_u64() || end.as_u64() > vma.end.as_u64() {
        return Err(AllocError::InvalidParams);
    }

    // Compute file offsets for each fragment.
    let before = if start.as_u64() > vma.start.as_u64() {
        let mut d = VmaDescriptor::new(vma.start, start, vma.flags, vma.page_flags, vma.backing);
        d.inode_id = vma.inode_id;
        d.file_offset = vma.file_offset;
        Some(d)
    } else {
        None
    };

    let mut middle = VmaDescriptor::new(start, end, vma.flags, new_flags, vma.backing);
    middle.inode_id = vma.inode_id;
    middle.file_offset = vma.file_offset + (start.as_u64() - vma.start.as_u64());

    let after = if end.as_u64() < vma.end.as_u64() {
        let mut d = VmaDescriptor::new(end, vma.end, vma.flags, vma.page_flags, vma.backing);
        d.inode_id = vma.inode_id;
        d.file_offset = vma.file_offset + (end.as_u64() - vma.start.as_u64());
        Some(d)
    } else {
        None
    };

    Ok(MprotectResult {
        before,
        middle,
        after,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// VMA VALIDATION
// ─────────────────────────────────────────────────────────────────────────────

/// Validates consistency of a VmaDescriptor prior to insertion.
pub fn validate_vma(vma: &VmaDescriptor) -> Result<(), AllocError> {
    let start = vma.start.as_u64();
    let end = vma.end.as_u64();
    if start >= end {
        return Err(AllocError::InvalidParams);
    }
    if !start.is_multiple_of(PAGE_SIZE as u64) {
        return Err(AllocError::InvalidParams);
    }
    if !end.is_multiple_of(PAGE_SIZE as u64) {
        return Err(AllocError::InvalidParams);
    }
    if vma.size() == 0 {
        return Err(AllocError::InvalidParams);
    }
    Ok(())
}
