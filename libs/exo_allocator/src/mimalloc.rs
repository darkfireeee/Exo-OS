//! Mimalloc FFI bindings and GlobalAlloc implementation

use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;

// FFI declarations for mimalloc
extern "C" {
    fn mi_malloc(size: usize) -> *mut c_void;
    fn mi_free(ptr: *mut c_void);
    fn mi_calloc(count: usize, size: usize) -> *mut c_void;
    fn mi_realloc(ptr: *mut c_void, newsize: usize) -> *mut c_void;
    fn mi_malloc_aligned(size: usize, alignment: usize) -> *mut c_void;
}

/// Mimalloc global allocator
pub struct Mimalloc;

unsafe impl GlobalAlloc for Mimalloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe {
            if layout.align() <= 16 {
                mi_malloc(layout.size()) as *mut u8
            } else {
                mi_malloc_aligned(layout.size(), layout.align()) as *mut u8
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe {
            mi_free(ptr as *mut c_void);
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe {
            mi_calloc(1, layout.size()) as *mut u8
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, _layout: Layout, new_size: usize) -> *mut u8 {
        unsafe {
            mi_realloc(ptr as *mut c_void, new_size) as *mut u8
        }
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
