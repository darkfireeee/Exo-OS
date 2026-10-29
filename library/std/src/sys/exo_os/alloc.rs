//! `std::sys::exo_os::alloc` — Global allocator backed by `exo-alloc` (`dlmalloc` pure Rust).

use core::alloc::{GlobalAlloc, Layout};

/// Exo-OS system allocator (`std::alloc::System`).
#[derive(Debug, Default, Clone, Copy)]
pub struct System;

unsafe impl GlobalAlloc for System {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { exo_alloc::ALLOCATOR.alloc(layout) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { exo_alloc::ALLOCATOR.alloc_zeroed(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { exo_alloc::ALLOCATOR.dealloc(ptr, layout) }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        unsafe { exo_alloc::ALLOCATOR.realloc(ptr, layout, new_size) }
    }
}
