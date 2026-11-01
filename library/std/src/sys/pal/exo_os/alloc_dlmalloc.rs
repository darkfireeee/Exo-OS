//! Pure-Rust `dlmalloc` allocator for Exo-OS (`SYS_MMAP = 9`, `SYS_MPROTECT = 10`,
//! `SYS_MUNMAP = 11`, `SYS_MREMAP = 25`), zero C symbols.

use crate::alloc::{GlobalAlloc, Layout};
use crate::ptr::null_mut;
use crate::sync::atomic::{AtomicBool, Ordering};
use super::syscall::{SYS_MMAP, SYS_MPROTECT, SYS_MREMAP, SYS_MUNMAP, decode_ret, syscall2, syscall3, syscall4, syscall6};

pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;

pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_FIXED: u64 = 0x10;
pub const MAP_ANONYMOUS: u64 = 0x20;

pub const MREMAP_MAYMOVE: u64 = 1;

pub const PAGE_SIZE: usize = 4096;
const ARENA_SIZE: usize = 16 * PAGE_SIZE; // 64 KiB per arena
const NUM_BINS: usize = 8; // 16, 32, 64, 128, 256, 512, 1024, 2048
const MAX_SMALL_SIZE: usize = 2048;

#[inline(always)]
pub const fn align_up(size: usize, align: usize) -> usize {
    let a = if align == 0 { 1 } else { align };
    (size + a - 1) & !(a - 1)
}

#[inline(always)]
pub unsafe fn sys_mmap(
    addr: usize,
    len: usize,
    prot: u64,
    flags: u64,
    fd: u64,
    offset: u64,
) -> Result<usize, i32> {
    let ret = unsafe { syscall6(SYS_MMAP, addr as u64, len as u64, prot, flags, fd, offset) };
    decode_ret(ret).map_err(|e| e.errno())
}

#[inline(always)]
pub unsafe fn sys_mprotect(addr: usize, len: usize, prot: u64) -> Result<usize, i32> {
    let ret = unsafe { syscall3(SYS_MPROTECT, addr as u64, len as u64, prot) };
    decode_ret(ret).map_err(|e| e.errno())
}

#[inline(always)]
pub unsafe fn sys_munmap(addr: usize, len: usize) -> Result<usize, i32> {
    let ret = unsafe { syscall2(SYS_MUNMAP, addr as u64, len as u64) };
    decode_ret(ret).map_err(|e| e.errno())
}

#[inline(always)]
pub unsafe fn sys_mremap(
    old_addr: usize,
    old_size: usize,
    new_size: usize,
    flags: u64,
) -> Result<usize, i32> {
    let ret = unsafe {
        syscall4(
            SYS_MREMAP,
            old_addr as u64,
            old_size as u64,
            new_size as u64,
            flags,
        )
    };
    decode_ret(ret).map_err(|e| e.errno())
}

#[repr(C)]
struct FreeNode {
    next: *mut FreeNode,
}

struct DlmallocState {
    bins: [*mut FreeNode; NUM_BINS],
    arena_cur: *mut u8,
    arena_end: *mut u8,
}

impl DlmallocState {
    const fn new() -> Self {
        Self {
            bins: [null_mut(); NUM_BINS],
            arena_cur: null_mut(),
            arena_end: null_mut(),
        }
    }

    #[inline(always)]
    fn bin_index(size: usize) -> usize {
        let s = size.max(16).next_power_of_two();
        (s.trailing_zeros() as usize).saturating_sub(4).min(NUM_BINS - 1)
    }

    #[inline(always)]
    fn bin_size(idx: usize) -> usize {
        16usize << idx
    }

    unsafe fn alloc_small(&mut self, size: usize) -> *mut u8 {
        let idx = Self::bin_index(size);
        let slot_size = Self::bin_size(idx);

        let head = self.bins[idx];
        if !head.is_null() {
            unsafe {
                self.bins[idx] = (*head).next;
                core::ptr::write_bytes(head as *mut u8, 0, slot_size);
            }
            return head as *mut u8;
        }

        let cur = self.arena_cur as usize;
        let end = self.arena_end as usize;
        let aligned_cur = align_up(cur, 16);
        if aligned_cur != 0 && aligned_cur.saturating_add(slot_size) <= end {
            self.arena_cur = (aligned_cur + slot_size) as *mut u8;
            return aligned_cur as *mut u8;
        }

        let new_arena = unsafe {
            match sys_mmap(
                0,
                ARENA_SIZE,
                PROT_READ | PROT_WRITE,
                MAP_ANONYMOUS | MAP_PRIVATE,
                !0,
                0,
            ) {
                Ok(ptr) => ptr,
                Err(_) => return null_mut(),
            }
        };
        self.arena_cur = (new_arena + slot_size) as *mut u8;
        self.arena_end = (new_arena + ARENA_SIZE) as *mut u8;
        new_arena as *mut u8
    }

    unsafe fn dealloc_small(&mut self, ptr: *mut u8, size: usize) {
        let idx = Self::bin_index(size);
        let node = ptr as *mut FreeNode;
        unsafe {
            (*node).next = self.bins[idx];
        }
        self.bins[idx] = node;
    }
}

static ALLOC_LOCK: AtomicBool = AtomicBool::new(false);
static mut DLMALLOC_STATE: DlmallocState = DlmallocState::new();

struct SpinGuard;

impl SpinGuard {
    #[inline(always)]
    fn lock() -> Self {
        while ALLOC_LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        Self
    }
}

impl Drop for SpinGuard {
    #[inline(always)]
    fn drop(&mut self) {
        ALLOC_LOCK.store(false, Ordering::Release);
    }
}

pub struct ExoDlmalloc;

impl ExoDlmalloc {
    pub const fn new() -> Self {
        Self
    }
}

unsafe impl GlobalAlloc for ExoDlmalloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return layout.align() as *mut u8;
        }
        if layout.size() <= MAX_SMALL_SIZE && layout.align() <= 16 {
            let _guard = SpinGuard::lock();
            return unsafe { (*core::ptr::addr_of_mut!(DLMALLOC_STATE)).alloc_small(layout.size()) };
        }
        let size = align_up(layout.size(), layout.align());
        let mmap_size = align_up(size, PAGE_SIZE);
        unsafe {
            match sys_mmap(
                0,
                mmap_size,
                PROT_READ | PROT_WRITE,
                MAP_ANONYMOUS | MAP_PRIVATE,
                !0,
                0,
            ) {
                Ok(ptr) => ptr as *mut u8,
                Err(_) => null_mut(),
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() || layout.size() == 0 {
            return;
        }
        if layout.size() <= MAX_SMALL_SIZE && layout.align() <= 16 {
            let _guard = SpinGuard::lock();
            unsafe {
                (*core::ptr::addr_of_mut!(DLMALLOC_STATE)).dealloc_small(ptr, layout.size());
            }
            return;
        }
        let size = align_up(layout.size(), layout.align());
        let mmap_size = align_up(size, PAGE_SIZE);
        unsafe {
            let _ = sys_munmap(ptr as usize, mmap_size);
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr.is_null() {
            return unsafe {
                self.alloc(Layout::from_size_align_unchecked(new_size, layout.align()))
            };
        }
        if new_size == 0 {
            unsafe { self.dealloc(ptr, layout) };
            return layout.align() as *mut u8;
        }
        let old_small = layout.size() <= MAX_SMALL_SIZE && layout.align() <= 16;
        let new_small = new_size <= MAX_SMALL_SIZE && layout.align() <= 16;

        if old_small && new_small {
            let old_idx = DlmallocState::bin_index(layout.size());
            let new_idx = DlmallocState::bin_index(new_size);
            if old_idx == new_idx {
                return ptr;
            }
        }

        if !old_small && !new_small {
            let old_size = align_up(layout.size(), layout.align());
            let old_mmap_size = align_up(old_size, PAGE_SIZE);
            let new_aligned = align_up(new_size, layout.align());
            let new_mmap_size = align_up(new_aligned, PAGE_SIZE);

            if old_mmap_size == new_mmap_size {
                return ptr;
            }
            unsafe {
                if let Ok(remapped) =
                    sys_mremap(ptr as usize, old_mmap_size, new_mmap_size, MREMAP_MAYMOVE)
                {
                    return remapped as *mut u8;
                }
            }
        }

        unsafe {
            let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
            let new_ptr = self.alloc(new_layout);
            if !new_ptr.is_null() {
                core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
            new_ptr
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { self.alloc(layout) }
    }
}

pub static GLOBAL_DLMALLOC: ExoDlmalloc = ExoDlmalloc::new();
