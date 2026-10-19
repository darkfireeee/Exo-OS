//! `exo-alloc` — Allocateur mémoire universel Ring 3 Exo-OS.
//!
//! Zéro symbole `libc` (pas de `malloc`, `free`, `sbrk`).
//! Allocation basée à 100% sur les appels système natifs `SYS_MMAP` (9),
//! `SYS_MUNMAP` (11) et `SYS_MREMAP` (25).
//!
//! Conforme au contrat formel `PhoenixSafe` (stateless, les pages `mmap`
//! survivent à la résurrection ExoPhoenix sans état mutable à restaurer).

#![no_std]

use core::alloc::{GlobalAlloc, Layout};
use core::marker::PhantomData;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicUsize, Ordering};
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

#[inline(always)]
pub fn align_up(size: usize, align: usize) -> usize {
    let a = align.max(1);
    (size + a - 1) & !(a - 1)
}

#[inline(always)]
unsafe fn sys_mmap(
    addr: usize,
    len: usize,
    prot: u64,
    flags: u64,
    fd: u64,
    offset: u64,
) -> Result<usize, i64> {
    let ret: i64;
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
    if ret < 0 && ret >= -4095 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

#[inline(always)]
unsafe fn sys_munmap(addr: usize, len: usize) -> Result<usize, i64> {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") SYS_MUNMAP => ret,
        in("rdi") addr as u64,
        in("rsi") len as u64,
        out("rcx") _,
        out("r11") _,
        options(nostack)
    );
    if ret < 0 && ret >= -4095 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

#[inline(always)]
unsafe fn sys_mremap(
    old_addr: usize,
    old_size: usize,
    new_size: usize,
    flags: u64,
) -> Result<usize, i64> {
    let ret: i64;
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
    if ret < 0 && ret >= -4095 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

/// Allocateur mémoire userland ExoOS universel.
/// Zéro dépendance libc, zéro `malloc`/`free`. Tout passe par `SYS_MMAP`/`SYS_MUNMAP`/`SYS_MREMAP`.
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
        let old_size = align_up(layout.size(), layout.align());
        let old_mmap_size = align_up(old_size, PAGE_SIZE);
        let new_aligned = align_up(new_size, layout.align());
        let new_mmap_size = align_up(new_aligned, PAGE_SIZE);

        if old_mmap_size == new_mmap_size {
            return ptr;
        }

        unsafe {
            if let Ok(remapped) = sys_mremap(ptr as usize, old_mmap_size, new_mmap_size, MREMAP_MAYMOVE) {
                return remapped as *mut u8;
            }
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
        Ok(())
    }

    fn on_post_switch(&self) -> Result<(), PhoenixError> {
        Ok(())
    }
}

/// Marqueur de fournisseur de heap pour compatibilité dlmalloc.
pub enum Exo {}

/// Façade pour compatibilité avec l'ancien nom de contrat v0.2.
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
        GLOBAL.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        GLOBAL.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        GLOBAL.realloc(ptr, layout, new_size)
    }
}

impl<P> PhoenixSafe for DlmallocAllocator<P> {
    const RECOVERY: PhoenixRecovery = PhoenixRecovery::Stateless;

    fn on_pre_switch(&self) -> Result<(), PhoenixError> {
        Ok(())
    }

    fn on_post_switch(&self) -> Result<(), PhoenixError> {
        Ok(())
    }
}

#[global_allocator]
pub static ALLOCATOR: DlmallocAllocator<Exo> = DlmallocAllocator::new();

pub static GLOBAL: ExoAllocator = ExoAllocator::new();

pub unsafe fn init_heap(heap_start: usize, heap_len: usize) {
    ALLOCATOR.init_heap(heap_start, heap_len);
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
}
