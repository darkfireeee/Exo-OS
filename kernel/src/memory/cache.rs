//! Cache management utilities
//! 
//! Provides cache line flushing and cache-aware data structures

use core::arch::asm;

/// Cache line size (x86_64 typically 64 bytes)
pub const CACHE_LINE_SIZE: usize = 64;

/// Cache-aligned wrapper type
#[repr(align(64))]
pub struct CacheAligned<T> {
    pub inner: T,
}

impl<T> CacheAligned<T> {
    pub const fn new(inner: T) -> Self {
        Self { inner }
    }
    
    pub fn get(&self) -> &T {
        &self.inner
    }
    
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

/// Flush cache line containing given address
#[inline]
pub fn clflush(addr: usize) {
    unsafe {
        asm!(
            "clflush [{}]",
            in(reg) addr,
            options(nostack)
        );
    }
}

/// Flush cache lines for a memory region
pub fn clflush_range(start: usize, size: usize) {
    let end = start + size;
    let mut addr = start & !(CACHE_LINE_SIZE - 1); // Align down
    
    while addr < end {
        clflush(addr);
        addr += CACHE_LINE_SIZE;
    }
    
    // Memory fence
    mfence();
}

/// Memory fence (serialize all loads and stores)
#[inline]
pub fn mfence() {
    unsafe {
        asm!("mfence", options(nostack));
    }
}

/// Load fence (serialize all loads)
#[inline]
pub fn lfence() {
    unsafe {
        asm!("lfence", options(nostack));
    }
}

/// Store fence (serialize all stores)
#[inline]
pub fn sfence() {
    unsafe {
        asm!("sfence", options(nostack));
    }
}

/// Prefetch data into cache (temporal locality)
#[inline]
pub fn prefetch_t0(addr: usize) {
    unsafe {
        asm!(
            "prefetcht0 [{}]",
            in(reg) addr,
            options(nostack)
        );
    }
}

/// Prefetch data into cache (low temporal locality)
#[inline]
pub fn prefetch_nta(addr: usize) {
    unsafe {
        asm!(
            "prefetchnta [{}]",
            in(reg) addr,
            options(nostack)
        );
    }
}

/// Cache statistics
#[derive(Debug, Default)]
pub struct CacheStats {
    pub flushes: usize,
    pub prefetches: usize,
}

/// Helper: Check if address is cache-aligned
pub const fn is_cache_aligned(addr: usize) -> bool {
    addr & (CACHE_LINE_SIZE - 1) == 0
}

/// Helper: Align address down to cache line
pub const fn align_down_to_cache_line(addr: usize) -> usize {
    addr & !(CACHE_LINE_SIZE - 1)
}

/// Helper: Align address up to cache line
pub const fn align_up_to_cache_line(addr: usize) -> usize {
    (addr + CACHE_LINE_SIZE - 1) & !(CACHE_LINE_SIZE - 1)
}
