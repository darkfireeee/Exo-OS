// kernel/src/memory/heap/allocator/global.rs
//
// Rust global allocator (#[global_allocator]) for the kernel.
// Delegates to the hybrid allocator (SLUB + large).
// Layer 0 — no external dependencies except `spin`.

use super::hybrid;
use crate::memory::core::AllocFlags;
use core::alloc::{GlobalAlloc, Layout};

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Rust global allocator for the kernel.
///
/// Registered via `#[global_allocator]` in lib.rs.
/// All Box, Vec, String allocations (if used) will pass through here.
///
/// Note: in the Exo-OS kernel, use of Box/Vec is forbidden in
/// no-alloc paths (IRQ, preempt-disabled). These allocations are tolerated
/// only in boot contexts and process management.
pub struct KernelAllocator;

// SAFETY: KernelAllocator is thread-safe — delegates to hybrid::alloc/free
//         which are themselves thread-safe (SLUB uses internal spinlocks).
unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match hybrid::alloc(layout.size(), layout.align(), AllocFlags::NONE) {
            Ok(ptr) => ptr.as_ptr(),
            Err(_) => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() {
            return;
        }
        if let Some(nn) = core::ptr::NonNull::new(ptr) {
            hybrid::free(nn, layout.size());
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let flags = AllocFlags::ZEROED;
        match hybrid::alloc(layout.size(), layout.align(), flags) {
            Ok(ptr) => ptr.as_ptr(),
            Err(_) => core::ptr::null_mut(),
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            return self.alloc(layout);
        }
        // Allocate the new block
        let new_layout = match Layout::from_size_align(new_size, layout.align()) {
            Ok(l) => l,
            Err(_) => return core::ptr::null_mut(),
        };
        let new_ptr = self.alloc(new_layout);
        if new_ptr.is_null() {
            return core::ptr::null_mut();
        }
        // Copy the data
        let copy_size = layout.size().min(new_size);
        // SAFETY: ptr is valid (previously allocated), copy_size <= both sizes.
        core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_size);
        // Free the old block
        self.dealloc(ptr, layout);
        new_ptr
    }
}

/// Global instance of the kernel allocator.
#[cfg_attr(not(test), global_allocator)]
pub static KERNEL_ALLOCATOR: KernelAllocator = KernelAllocator;
