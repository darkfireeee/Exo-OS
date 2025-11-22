//! Heap allocator

use core::alloc::{GlobalAlloc, Layout};
use spin::Mutex;

pub struct LockedHeap {
    inner: Mutex<()>,
}

impl LockedHeap {
    pub const fn empty() -> Self {
        LockedHeap {
            inner: Mutex::new(()),
        }
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, _layout: Layout) -> *mut u8 {
        core::ptr::null_mut()
    }
    
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}
