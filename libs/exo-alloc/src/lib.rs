//! `exo-alloc` — Allocateur mémoire universel Ring 3 Exo-OS (dlmalloc pur Rust).
//!
//! Zéro symbole `libc` (pas de `malloc`, `free`, `sbrk` externes).
//! Allocation basée à 100% sur les appels système natifs `SYS_MMAP` (9),
//! `SYS_MPROTECT` (10), `SYS_MUNMAP` (11) et `SYS_MREMAP` (25).
//!
//! Architecture :
//! - Petites allocations (`<= 2048` octets, alignement `<= 16`) : classes de tailles
//!   ségréguées (16, 32, 64, 128, 256, 512, 1024, 2048 octets) découpées dans des
//!   arènes `mmap` de 64 KiB avec listes libres intrusives O(1).
//! - Allocations moyennes/grandes (`> 2048` octets ou alignement `> 16`) : pages
//!   directes `SYS_MMAP` / `SYS_MREMAP` / `SYS_MUNMAP`.
//! - Conforme au contrat formel `PhoenixSafe` (les pages `mmap` survivent à la
//!   résurrection ExoPhoenix).

#![no_std]

use core::alloc::{GlobalAlloc, Layout};
use core::marker::PhantomData;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use exo_phoenix_ssr::{PhoenixError, PhoenixRecovery, PhoenixSafe};

pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;

pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_FIXED: u64 = 0x10;
pub const MAP_ANONYMOUS: u64 = 0x20;

pub const MREMAP_MAYMOVE: u64 = 1;

pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_MREMAP: u64 = 25;

pub const PAGE_SIZE: usize = 4096;
const ARENA_SIZE: usize = 16 * PAGE_SIZE; // 64 KiB par arène dlmalloc
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
) -> Result<usize, i64> {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_MMAP => ret,
            in("rdi") addr as u64,
            in("rsi") len as u64,
            in("rdx") prot,
            in("r10") flags,
            in("r8") fd,
            in("r9") offset,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    if ret < 0 && ret >= -4095 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

#[inline(always)]
pub unsafe fn sys_mprotect(addr: usize, len: usize, prot: u64) -> Result<usize, i64> {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_MPROTECT => ret,
            in("rdi") addr as u64,
            in("rsi") len as u64,
            in("rdx") prot,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    if ret < 0 && ret >= -4095 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

#[inline(always)]
pub unsafe fn sys_munmap(addr: usize, len: usize) -> Result<usize, i64> {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_MUNMAP => ret,
            in("rdi") addr as u64,
            in("rsi") len as u64,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    if ret < 0 && ret >= -4095 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

#[inline(always)]
pub unsafe fn sys_mremap(
    old_addr: usize,
    old_size: usize,
    new_size: usize,
    flags: u64,
) -> Result<usize, i64> {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_MREMAP => ret,
            in("rdi") old_addr as u64,
            in("rsi") old_size as u64,
            in("rdx") new_size as u64,
            in("r10") flags,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    if ret < 0 && ret >= -4095 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
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

        // Allouer une nouvelle arène de 64 KiB via SYS_MMAP
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

/// Allocateur mémoire userland ExoOS universel (dlmalloc pur Rust sur mmap/mremap/munmap).
/// Zéro dépendance libc, zéro symbole C `malloc`/`free`.
pub struct ExoAllocator;

impl ExoAllocator {
    pub const fn new() -> Self {
        Self
    }
}

unsafe impl GlobalAlloc for ExoAllocator {
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
            let new_ptr = self.alloc(Layout::from_size_align_unchecked(new_size, layout.align()));
            if !new_ptr.is_null() {
                core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
            new_ptr
        }
    }
}

impl PhoenixSafe for ExoAllocator {
    const RECOVERY: PhoenixRecovery = PhoenixRecovery::Stateless;

    fn on_pre_switch(&self) -> Result<(), PhoenixError> {
        core::sync::atomic::fence(Ordering::SeqCst);
        Ok(())
    }

    fn on_post_switch(&self) -> Result<(), PhoenixError> {
        ALLOC_LOCK.store(false, Ordering::Release);
        core::sync::atomic::fence(Ordering::SeqCst);
        Ok(())
    }
}

/// Marqueur de fournisseur de heap pour compatibilité dlmalloc.
pub enum Exo {}

/// Façade pour compatibilité avec le contrat `DlmallocAllocator<Exo>`.
pub struct DlmallocAllocator<P> {
    start: AtomicUsize,
    end: AtomicUsize,
    _provider: PhantomData<P>,
}

impl<P> DlmallocAllocator<P> {
    pub const fn new() -> Self {
        Self {
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(0),
            _provider: PhantomData,
        }
    }

    pub unsafe fn init_heap(&self, heap_start: usize, heap_len: usize) {
        let heap_end = heap_start.saturating_add(heap_len);
        self.start.store(heap_start, Ordering::Release);
        self.end.store(heap_end, Ordering::Release);
    }

    #[inline]
    pub fn heap_bounds(&self) -> Option<(usize, usize)> {
        let start = self.start.load(Ordering::Acquire);
        let end = self.end.load(Ordering::Acquire);
        if start == 0 || start >= end {
            None
        } else {
            Some((start, end))
        }
    }
}

unsafe impl<P> Sync for DlmallocAllocator<P> {}

unsafe impl<P> GlobalAlloc for DlmallocAllocator<P> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { GLOBAL.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { GLOBAL.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        unsafe { GLOBAL.realloc(ptr, layout, new_size) }
    }
}

impl<P> PhoenixSafe for DlmallocAllocator<P> {
    const RECOVERY: PhoenixRecovery = PhoenixRecovery::Stateless;

    fn on_pre_switch(&self) -> Result<(), PhoenixError> {
        GLOBAL.on_pre_switch()
    }

    fn on_post_switch(&self) -> Result<(), PhoenixError> {
        GLOBAL.on_post_switch()
    }
}

#[cfg(all(not(test), any(target_os = "none", target_os = "exo", feature = "global-alloc")))]
#[global_allocator]
pub static ALLOCATOR: DlmallocAllocator<Exo> = DlmallocAllocator::new();

#[cfg(any(test, not(any(target_os = "none", target_os = "exo", feature = "global-alloc"))))]
pub static ALLOCATOR: DlmallocAllocator<Exo> = DlmallocAllocator::new();

pub static GLOBAL: ExoAllocator = ExoAllocator::new();

pub unsafe fn init_heap(heap_start: usize, heap_len: usize) {
    unsafe {
        ALLOCATOR.init_heap(heap_start, heap_len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_align_up() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 8), 16);
        assert_eq!(align_up(4095, 4096), 4096);
        assert_eq!(align_up(4096, 4096), 4096);
        assert_eq!(align_up(4097, 4096), 8192);
    }

    #[test]
    fn test_phoenix_safe() {
        assert!(GLOBAL.is_stateless());
        assert_eq!(GLOBAL.on_pre_switch(), Ok(()));
        assert_eq!(GLOBAL.on_post_switch(), Ok(()));
    }

    #[test]
    fn test_small_and_large_alloc_dealloc() {
        unsafe {
            let layout_small = Layout::from_size_align(64, 8).unwrap();
            let p1 = GLOBAL.alloc(layout_small);
            assert!(!p1.is_null());
            core::ptr::write_bytes(p1, 0xAA, 64);
            let p2 = GLOBAL.realloc(p1, layout_small, 256);
            assert!(!p2.is_null());
            assert_eq!(*p2, 0xAA);
            GLOBAL.dealloc(p2, Layout::from_size_align(256, 8).unwrap());

            let layout_large = Layout::from_size_align(8192, 16).unwrap();
            let p3 = GLOBAL.alloc(layout_large);
            assert!(!p3.is_null());
            core::ptr::write_bytes(p3, 0x55, 8192);
            GLOBAL.dealloc(p3, layout_large);
        }
    }
}
