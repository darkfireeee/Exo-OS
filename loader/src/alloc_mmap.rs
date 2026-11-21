//! Minimal `#![no_std]` `mmap`-backed global allocator and spinlock primitive for `/lib/ld-exo.so`.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::syscall::{sys_mmap, sys_munmap, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE};

const PAGE_SIZE: usize = 4096;
const SLAB_BYTES: usize = 16 * PAGE_SIZE; // 64 KiB slab per growth
const NUM_CLASSES: usize = 8;
const CLASS_SIZES: [usize; NUM_CLASSES] = [16, 32, 64, 128, 256, 512, 1024, 2048];
const ALLOC_HEADER_ALIGN: usize = 16;

/// Minimal ticket/TAS spinlock for loader metadata synchronization.
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    #[inline]
    pub fn lock(&self) -> SpinGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinGuard { lock: self }
    }
}

pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

#[repr(C)]
struct FreeNode {
    next: *mut FreeNode,
}

#[repr(C)]
struct LargeHeader {
    map_bytes: usize,
    _pad: usize,
}

const _: () = assert!(core::mem::size_of::<LargeHeader>() == ALLOC_HEADER_ALIGN);

struct InnerAlloc {
    freelists: [*mut FreeNode; NUM_CLASSES],
    bump_cur: usize,
    bump_end: usize,
}

unsafe impl Send for InnerAlloc {}

impl InnerAlloc {
    const fn new() -> Self {
        Self {
            freelists: [ptr::null_mut(); NUM_CLASSES],
            bump_cur: 0,
            bump_end: 0,
        }
    }

    fn size_class(size: usize, align: usize) -> Option<usize> {
        let req = size.max(align);
        let mut idx = 0usize;
        while idx < NUM_CLASSES {
            if CLASS_SIZES[idx] >= req {
                return Some(idx);
            }
            idx += 1;
        }
        None
    }

    unsafe fn alloc_class(&mut self, class_idx: usize) -> *mut u8 {
        let head = self.freelists[class_idx];
        if !head.is_null() {
            unsafe {
                self.freelists[class_idx] = (*head).next;
            }
            return head as *mut u8;
        }

        let block_size = CLASS_SIZES[class_idx];
        let aligned_cur = (self.bump_cur + block_size - 1) & !(block_size - 1);
        if aligned_cur.saturating_add(block_size) <= self.bump_end {
            self.bump_cur = aligned_cur + block_size;
            return aligned_cur as *mut u8;
        }

        let Ok(slab) = (unsafe {
            sys_mmap(
                0,
                SLAB_BYTES,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        }) else {
            return ptr::null_mut();
        };
        let ptr_start = (slab + block_size - 1) & !(block_size - 1);
        self.bump_cur = ptr_start + block_size;
        self.bump_end = slab + SLAB_BYTES;
        ptr_start as *mut u8
    }

    unsafe fn dealloc_class(&mut self, ptr: *mut u8, class_idx: usize) {
        let node = ptr as *mut FreeNode;
        unsafe {
            (*node).next = self.freelists[class_idx];
        }
        self.freelists[class_idx] = node;
    }
}

pub struct MmapLoaderAlloc {
    inner: SpinLock<InnerAlloc>,
}

impl MmapLoaderAlloc {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(InnerAlloc::new()),
        }
    }
}

unsafe impl GlobalAlloc for MmapLoaderAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return NonNull::<u8>::dangling().as_ptr();
        }
        if layout.align() <= ALLOC_HEADER_ALIGN {
            if let Some(class_idx) = InnerAlloc::size_class(layout.size(), layout.align()) {
                return unsafe { self.inner.lock().alloc_class(class_idx) };
            }
        }

        let header_pad = layout.align().max(ALLOC_HEADER_ALIGN);
        let Some(total_raw) = header_pad.checked_add(layout.size()) else {
            return ptr::null_mut();
        };
        let map_bytes = (total_raw + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let Ok(base) = (unsafe {
            sys_mmap(
                0,
                map_bytes,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        }) else {
            return ptr::null_mut();
        };
        let user_addr = base + header_pad;
        let hdr = (user_addr - core::mem::size_of::<LargeHeader>()) as *mut LargeHeader;
        unsafe {
            (*hdr).map_bytes = map_bytes;
            (*hdr)._pad = base;
        }
        user_addr as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if layout.size() == 0 || ptr.is_null() {
            return;
        }
        if layout.align() <= ALLOC_HEADER_ALIGN {
            if let Some(class_idx) = InnerAlloc::size_class(layout.size(), layout.align()) {
                unsafe {
                    self.inner.lock().dealloc_class(ptr, class_idx);
                }
                return;
            }
        }
        let hdr = (ptr as usize - core::mem::size_of::<LargeHeader>()) as *const LargeHeader;
        let map_base = unsafe { (*hdr)._pad };
        let map_bytes = unsafe { (*hdr).map_bytes };
        unsafe {
            let _ = sys_munmap(map_base, map_bytes);
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return ptr::null_mut();
        };
        let new_ptr = unsafe { self.alloc(new_layout) };
        if new_ptr.is_null() {
            return ptr::null_mut();
        }
        let copy_len = layout.size().min(new_size);
        unsafe {
            ptr::copy_nonoverlapping(ptr, new_ptr, copy_len);
            self.dealloc(ptr, layout);
        }
        new_ptr
    }
}
