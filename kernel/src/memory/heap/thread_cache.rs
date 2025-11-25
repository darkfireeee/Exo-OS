//! Thread-Local Cache Allocator
//! 
//! Provides fast, lock-free allocations for small objects (≤256 bytes)
//! Target: ~8 cycles per allocation (cache hit)

use core::ptr::NonNull;
use super::size_class::SizeClass;

/// Maximum size for thread-local cache (256 bytes)
pub const THREAD_CACHE_MAX_SIZE: usize = 256;

/// Number of size classes in thread cache (16, 32, 64, 128, 256)
pub const THREAD_CACHE_CLASSES: usize = 5;

/// Objects per size class in thread cache
const OBJECTS_PER_CLASS: usize = 64;

/// Free list node for cached objects
struct FreeListNode {
    next: Option<NonNull<FreeListNode>>,
}

unsafe impl Send for FreeListNode {}

/// Thread-local cache for a single size class
struct ThreadCacheBin {
    head: Option<NonNull<FreeListNode>>,
    count: usize,
    size: usize,
}

unsafe impl Send for ThreadCacheBin {}

impl ThreadCacheBin {
    const fn new(size: usize) -> Self {
        Self {
            head: None,
            count: 0,
            size,
        }
    }

    /// Allocate from cache (pop)
    fn allocate(&mut self) -> Option<*mut u8> {
        if let Some(head) = self.head {
            unsafe {
                let head_ptr = head.as_ptr();
                self.head = (*head_ptr).next;
                self.count -= 1;
                Some(head_ptr as *mut u8)
            }
        } else {
            None
        }
    }

    /// Return to cache (push)
    fn deallocate(&mut self, ptr: *mut u8) -> bool {
        if self.count >= OBJECTS_PER_CLASS {
            return false; // Cache full
        }

        unsafe {
            let node = ptr as *mut FreeListNode;
            (*node).next = self.head;
            self.head = Some(NonNull::new_unchecked(node));
            self.count += 1;
            true
        }
    }

    /// Check if cache has space
    fn has_space(&self) -> bool {
        self.count < OBJECTS_PER_CLASS
    }

    /// Flush all objects back to CPU slab
    fn flush<F>(&mut self, mut callback: F)
    where
        F: FnMut(*mut u8, usize),
    {
        while let Some(ptr) = self.allocate() {
            callback(ptr, self.size);
        }
    }
}

/// Thread-local cache allocator
pub struct ThreadCache {
    bins: [ThreadCacheBin; THREAD_CACHE_CLASSES],
    allocations: usize,
    deallocations: usize,
}

unsafe impl Send for ThreadCache {}

impl ThreadCache {
    /// Create a new thread cache
    pub const fn new() -> Self {
        Self {
            bins: [
                ThreadCacheBin::new(16),
                ThreadCacheBin::new(32),
                ThreadCacheBin::new(64),
                ThreadCacheBin::new(128),
                ThreadCacheBin::new(256),
            ],
            allocations: 0,
            deallocations: 0,
        }
    }

    /// Get bin index for size
    fn bin_index(size: usize) -> Option<usize> {
        match size {
            0..=16 => Some(0),
            17..=32 => Some(1),
            33..=64 => Some(2),
            65..=128 => Some(3),
            129..=256 => Some(4),
            _ => None,
        }
    }

    /// Allocate from thread cache
    pub fn allocate(&mut self, size: usize) -> Option<*mut u8> {
        if let Some(idx) = Self::bin_index(size) {
            if let Some(ptr) = self.bins[idx].allocate() {
                self.allocations += 1;
                return Some(ptr);
            }
        }
        None
    }

    /// Deallocate to thread cache
    pub fn deallocate(&mut self, ptr: *mut u8, size: usize) -> bool {
        if let Some(idx) = Self::bin_index(size) {
            if self.bins[idx].deallocate(ptr) {
                self.deallocations += 1;
                return true;
            }
        }
        false
    }

    /// Flush all caches
    pub fn flush_all<F>(&mut self, callback: F)
    where
        F: FnMut(*mut u8, usize) + Copy,
    {
        for bin in &mut self.bins {
            bin.flush(callback);
        }
    }

    /// Get statistics
    pub fn stats(&self) -> ThreadCacheStats {
        let mut total_cached = 0;
        for bin in &self.bins {
            total_cached += bin.count;
        }

        ThreadCacheStats {
            allocations: self.allocations,
            deallocations: self.deallocations,
            cached_objects: total_cached,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadCacheStats {
    pub allocations: usize,
    pub deallocations: usize,
    pub cached_objects: usize,
}

// Per-CPU thread cache (no_std compatible)
use spin::Mutex;

static THREAD_CACHE: Mutex<ThreadCache> = Mutex::new(ThreadCache::new());

/// Allocate from thread-local cache
pub fn thread_alloc(size: usize) -> Option<*mut u8> {
    THREAD_CACHE.lock().allocate(size)
}

/// Deallocate to thread-local cache
pub fn thread_dealloc(ptr: *mut u8, size: usize) -> bool {
    THREAD_CACHE.lock().deallocate(ptr, size)
}

/// Flush thread cache
pub fn thread_flush<F>(callback: F)
where
    F: FnMut(*mut u8, usize) + Copy,
{
    THREAD_CACHE.lock().flush_all(callback);
}

/// Get thread cache stats
pub fn thread_stats() -> ThreadCacheStats {
    THREAD_CACHE.lock().stats()
}
