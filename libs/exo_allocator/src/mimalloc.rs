//! Pure-Rust GlobalAlloc implementation backed by `exo-alloc` (`dlmalloc` on `SYS_MMAP`).
//! Replaces the legacy C `mimalloc` FFI so zero C symbols exist in `libs/`.

use core::alloc::{GlobalAlloc, Layout};

/// General-purpose global allocator (backed by pure-Rust `exo_alloc::GLOBAL`).
pub struct Mimalloc;

unsafe impl GlobalAlloc for Mimalloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { exo_alloc::GLOBAL.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { exo_alloc::GLOBAL.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { exo_alloc::GLOBAL.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        unsafe { exo_alloc::GLOBAL.realloc(ptr, layout, new_size) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mimalloc_compiles() {
        let _ = Mimalloc;
    }
}
