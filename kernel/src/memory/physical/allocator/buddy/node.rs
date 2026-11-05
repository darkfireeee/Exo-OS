//! # memory/physical/allocator/buddy/node.rs — Node structure and free-list levels
//!
//! Doubly linked nodes stored in-place within free physical pages
//! and alignment/address translation management via physmap.

use core::ptr::null_mut;
use core::sync::atomic::AtomicUsize;
use crate::memory::core::{PAGE_SIZE, PhysAddr};

// ─────────────────────────────────────────────────────────────────────────────
// FREE LIST NODE — linked list node in a free buddy block
// ─────────────────────────────────────────────────────────────────────────────

/// Linked list node embedded in a free block.
/// Stored directly in the first bytes of the block (free physical page).
///
/// SAFETY: This node is readable only when the block is FREE.
/// Once a block is allocated, its contents are undefined for the allocator.
#[repr(C)]
pub struct FreeNode {
    pub next: *mut FreeNode,
    pub prev: *mut FreeNode,
    pub order: u8,
    pub _pad: [u8; 7],
}

impl FreeNode {
    /// Initializes the sentinel or standalone node.
    ///
    /// # Safety
    /// The pointer `ptr` must be valid and properly aligned.
    #[inline]
    pub unsafe fn init(ptr: *mut FreeNode, order: u8) {
        (*ptr).next = ptr;
        (*ptr).prev = ptr;
        (*ptr).order = order;
    }

    /// Inserts a node after the given list head.
    ///
    /// # Safety
    /// `list_head` and `node` must be valid and dereferenceable pointers.
    #[inline]
    pub unsafe fn insert_after(list_head: *mut FreeNode, node: *mut FreeNode) {
        let next = (*list_head).next;
        (*node).prev = list_head;
        (*node).next = next;
        (*next).prev = node;
        (*list_head).next = node;
    }

    /// Removes the node from its doubly linked list.
    ///
    /// # Safety
    /// `node` must be a valid pointer currently inserted in a list.
    #[inline]
    pub unsafe fn remove(node: *mut FreeNode) {
        let prev = (*node).prev;
        let next = (*node).next;
        (*prev).next = next;
        (*next).prev = prev;
        (*node).next = node;
        (*node).prev = node;
    }

    /// Checks if the list is empty.
    ///
    /// # Safety
    /// `list_head` must be null or a valid pointer to a list sentinel.
    #[inline]
    pub unsafe fn is_empty(list_head: *const FreeNode) -> bool {
        if list_head.is_null() {
            return true;
        }
        core::ptr::eq((*list_head).next, list_head)
    }

    /// Removes and returns the first element of the list.
    ///
    /// # Safety
    /// `list_head` must point to a valid list sentinel.
    #[inline]
    pub unsafe fn pop(list_head: *mut FreeNode) -> Option<*mut FreeNode> {
        if Self::is_empty(list_head) {
            return None;
        }
        let node = (*list_head).next;
        Self::remove(node);
        Some(node)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BUDDY ORDER LEVEL — one free-list for a given order
// ─────────────────────────────────────────────────────────────────────────────

/// Free-list head for a given order.
#[repr(C, align(64))]
pub struct BuddyOrderLevel {
    /// Sentinel head (empty node — next/prev point to itself if empty).
    pub head: FreeNode,
    /// Number of free blocks at this order.
    pub count: AtomicUsize,
}

impl Default for BuddyOrderLevel {
    fn default() -> Self {
        Self::new()
    }
}

impl BuddyOrderLevel {
    pub const fn new() -> Self {
        BuddyOrderLevel {
            head: FreeNode {
                next: null_mut(),
                prev: null_mut(),
                order: 0,
                _pad: [0; 7],
            },
            count: AtomicUsize::new(0),
        }
    }

    /// Initializes sentinel (cannot be done in const, requires pointer).
    ///
    /// # Safety
    /// `self` must be pinned or stored at a stable final memory address.
    pub unsafe fn init(&mut self) {
        let h = &mut self.head as *mut FreeNode;
        (*h).next = h;
        (*h).prev = h;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// INTERNAL UTILITIES (direct physmap bridge)
// ─────────────────────────────────────────────────────────────────────────────

/// Translates a physical address to a virtual pointer to access FreeNodes.
/// Uses direct physmap (PHYS_MAP_BASE + phys).
#[inline(always)]
pub fn phys_to_virt_buddy(phys: PhysAddr) -> usize {
    use crate::memory::core::layout::PHYS_MAP_BASE;
    PHYS_MAP_BASE.as_usize() + phys.as_usize()
}

/// Translates a virtual pointer (in physmap) to a physical address.
#[inline(always)]
pub fn virt_to_phys_buddy(virt: usize) -> PhysAddr {
    use crate::memory::core::layout::PHYS_MAP_BASE;
    PhysAddr::new((virt - PHYS_MAP_BASE.as_usize()) as u64)
}

/// Fills a page range with zeroes.
///
/// # Safety
/// `phys` must be the address of `2^order` pages allocated exclusively
/// by caller. Physmap must be accessible.
#[inline]
pub unsafe fn zero_pages(phys: PhysAddr, order: usize) {
    let size = PAGE_SIZE << order;
    let virt = phys_to_virt_buddy(phys) as *mut u8;
    core::ptr::write_bytes(virt, 0, size);
}
