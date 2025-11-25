//! CPU-Local Slab Allocator
//! 
//! Provides per-CPU allocations for medium objects (≤4KB)
//! Target: ~50 cycles per allocation
//! Uses atomic operations for thread safety within CPU

use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use core::ptr::{self, NonNull};
use spin::Mutex;

/// Maximum size for CPU slab (4KB)
pub const CPU_SLAB_MAX_SIZE: usize = 4096;

/// Number of size classes (512, 1024, 2048, 4096)
pub const CPU_SLAB_CLASSES: usize = 4;

/// Objects per slab
const OBJECTS_PER_SLAB: usize = 32;

/// Atomic free list for lock-free operations
struct AtomicFreeList {
    head: AtomicPtr<FreeListNode>,
    count: AtomicUsize,
    size: usize,
}

struct FreeListNode {
    next: *mut FreeListNode,
}

impl AtomicFreeList {
    const fn new(size: usize) -> Self {
        Self {
            head: AtomicPtr::new(ptr::null_mut()),
            count: AtomicUsize::new(0),
            size,
        }
    }

    /// Push to free list (lock-free)
    fn push(&self, ptr: *mut u8) {
        let node = ptr as *mut FreeListNode;
        
        loop {
            let head = self.head.load(Ordering::Acquire);
            
            unsafe {
                (*node).next = head;
            }
            
            if self.head.compare_exchange(
                head,
                node,
                Ordering::Release,
                Ordering::Acquire,
            ).is_ok() {
                self.count.fetch_add(1, Ordering::Release);
                break;
            }
        }
    }

    /// Pop from free list (lock-free)
    fn pop(&self) -> Option<*mut u8> {
        loop {
            let head = self.head.load(Ordering::Acquire);
            
            if head.is_null() {
                return None;
            }
            
            let next = unsafe { (*head).next };
            
            if self.head.compare_exchange(
                head,
                next,
                Ordering::Release,
                Ordering::Acquire,
            ).is_ok() {
                self.count.fetch_sub(1, Ordering::Release);
                return Some(head as *mut u8);
            }
        }
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

/// CPU-local slab allocator
pub struct CpuSlab {
    slabs: [AtomicFreeList; CPU_SLAB_CLASSES],
    allocations: AtomicUsize,
    deallocations: AtomicUsize,
}

impl CpuSlab {
    /// Create a new CPU slab
    pub const fn new() -> Self {
        Self {
            slabs: [
                AtomicFreeList::new(512),
                AtomicFreeList::new(1024),
                AtomicFreeList::new(2048),
                AtomicFreeList::new(4096),
            ],
            allocations: AtomicUsize::new(0),
            deallocations: AtomicUsize::new(0),
        }
    }

    /// Get slab index for size
    fn slab_index(size: usize) -> Option<usize> {
        match size {
            0..=512 => Some(0),
            513..=1024 => Some(1),
            1025..=2048 => Some(2),
            2049..=4096 => Some(3),
            _ => None,
        }
    }

    /// Allocate from CPU slab
    pub fn allocate(&self, size: usize) -> Option<*mut u8> {
        if let Some(idx) = Self::slab_index(size) {
            if let Some(ptr) = self.slabs[idx].pop() {
                self.allocations.fetch_add(1, Ordering::Relaxed);
                return Some(ptr);
            }
        }
        None
    }

    /// Deallocate to CPU slab
    pub fn deallocate(&self, ptr: *mut u8, size: usize) -> bool {
        if let Some(idx) = Self::slab_index(size) {
            self.slabs[idx].push(ptr);
            self.deallocations.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Get statistics
    pub fn stats(&self) -> CpuSlabStats {
        let mut total_cached = 0;
        for slab in &self.slabs {
            total_cached += slab.count();
        }

        CpuSlabStats {
            allocations: self.allocations.load(Ordering::Relaxed),
            deallocations: self.deallocations.load(Ordering::Relaxed),
            cached_objects: total_cached,
        }
    }
}

unsafe impl Send for CpuSlab {}
unsafe impl Sync for CpuSlab {}

#[derive(Debug, Clone, Copy)]
pub struct CpuSlabStats {
    pub allocations: usize,
    pub deallocations: usize,
    pub cached_objects: usize,
}

/// Global CPU slab (one per CPU, for now just one global)
static CPU_SLAB: CpuSlab = CpuSlab::new();

/// Allocate from CPU slab
pub fn cpu_alloc(size: usize) -> Option<*mut u8> {
    CPU_SLAB.allocate(size)
}

/// Deallocate to CPU slab
pub fn cpu_dealloc(ptr: *mut u8, size: usize) -> bool {
    CPU_SLAB.deallocate(ptr, size)
}

/// Get CPU slab stats
pub fn cpu_stats() -> CpuSlabStats {
    CPU_SLAB.stats()
}
