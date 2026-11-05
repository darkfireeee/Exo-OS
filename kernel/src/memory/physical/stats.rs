// kernel/src/memory/physical/stats.rs
//
//
// PHYSICAL STATISTICS  wrappers towards the buddy allocator
//
//
// This module exposes global statistics of the physical memory subsystem.
// Used by syscalls to implement sysinfo/mem_info.
//
// LAYER 0  no external dependencies.

use crate::memory::physical::BUDDY;

pub use crate::memory::core::PAGE_SIZE;

/// Returns the number of free physical pages.
#[inline]
pub fn free_pages() -> usize {
    BUDDY.total_free_frames()
}

/// Returns the total number of physical pages managed by the kernel.
#[inline]
pub fn total_pages() -> usize {
    BUDDY.total_frames()
}

/// Returns the amount of free memory in bytes.
#[inline]
pub fn free_bytes() -> usize {
    free_pages() * PAGE_SIZE
}

/// Returns the total amount of physical memory in bytes.
#[inline]
pub fn total_bytes() -> usize {
    total_pages() * PAGE_SIZE
}

/// Returns the number of used (allocated) pages.
#[inline]
pub fn used_pages() -> usize {
    total_pages().saturating_sub(free_pages())
}
