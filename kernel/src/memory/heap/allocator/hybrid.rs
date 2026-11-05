// kernel/src/memory/heap/allocator/hybrid.rs
//
// Hybrid heap allocator — dispatch between SLUB (<= 2 KiB) and large (> 2 KiB).
// Single interface for the kernel heap.
// Layer 0 — no external dependencies except `spin`.

use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::size_classes::{heap_size_class_for, HEAP_LARGE_THRESHOLD, HEAP_SIZE_CLASSES};
use crate::memory::core::{AllocError, AllocFlags};
use crate::memory::physical::allocator::slub::SLUB_CACHES;

// ─────────────────────────────────────────────────────────────────────────────
// STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct HybridAllocStats {
    pub small_allocs: AtomicU64,
    pub small_frees: AtomicU64,
    pub large_allocs: AtomicU64,
    pub large_frees: AtomicU64,
    pub oom_count: AtomicU64,
    pub current_inuse: AtomicU64,
}

impl Default for HybridAllocStats {
    fn default() -> Self {
        Self::new()
    }
}

impl HybridAllocStats {
    pub const fn new() -> Self {
        HybridAllocStats {
            small_allocs: AtomicU64::new(0),
            small_frees: AtomicU64::new(0),
            large_allocs: AtomicU64::new(0),
            large_frees: AtomicU64::new(0),
            oom_count: AtomicU64::new(0),
            current_inuse: AtomicU64::new(0),
        }
    }
}

pub static HEAP_STATS: HybridAllocStats = HybridAllocStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// HYBRID ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Allocator backend used (SLUB or Large).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocBackend {
    Slub,
    Large,
}

static HYBRID_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    HYBRID_ENABLED.store(true, Ordering::Release);
}

/// Returns true when the kernel heap allocator accepts allocations.
#[inline]
pub fn is_heap_ready() -> bool {
    HYBRID_ENABLED.load(Ordering::Acquire)
}

/// Allocates `size` bytes with minimum `align`.
///
/// - If size <= 2048: uses SLUB.
/// - Otherwise: uses large allocator (vmalloc).
pub fn alloc(size: usize, align: usize, flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
    if !HYBRID_ENABLED.load(Ordering::Acquire) {
        return Err(AllocError::NotInitialized);
    }

    let user_size = if size == 0 { 8 } else { size };

    // T5-001 FIX: If requested alignment exceeds 64 bytes, route to vmalloc
    // which guarantees PAGE_SIZE (4096 bytes) alignment.
    if user_size <= HEAP_LARGE_THRESHOLD && align <= 64 {
        // Small allocation via SLUB
        let sc_entry = heap_size_class_for(user_size).ok_or(AllocError::InvalidParams)?;
        let slab_idx = HEAP_SIZE_CLASSES[sc_entry].slab_idx;
        match SLUB_CACHES[slab_idx].alloc(flags) {
            Ok(ptr) => {
                HEAP_STATS.small_allocs.fetch_add(1, Ordering::Relaxed);
                HEAP_STATS
                    .current_inuse
                    .fetch_add(user_size as u64, Ordering::Relaxed);
                Ok(ptr)
            }
            Err(_e) => {
                // Robustness fallback: if SLUB is not yet ready or fails
                // intermittently, attempt large allocator to avoid early panic
                // during boot.
                match crate::memory::heap::large::vmalloc::kalloc(user_size, flags) {
                    Ok(ptr) => {
                        HEAP_STATS.large_allocs.fetch_add(1, Ordering::Relaxed);
                        HEAP_STATS
                            .current_inuse
                            .fetch_add(user_size as u64, Ordering::Relaxed);
                        Ok(ptr)
                    }
                    Err(e2) => {
                        HEAP_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
                        Err(e2)
                    }
                }
            }
        }
    } else {
        // Large allocation via large allocator
        match crate::memory::heap::large::vmalloc::kalloc(user_size, flags) {
            Ok(ptr) => {
                HEAP_STATS.large_allocs.fetch_add(1, Ordering::Relaxed);
                HEAP_STATS
                    .current_inuse
                    .fetch_add(user_size as u64, Ordering::Relaxed);
                Ok(ptr)
            }
            Err(e) => {
                HEAP_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }
}

/// Frees a pointer allocated by `alloc`.
///
/// # Safety
/// `ptr` must have been allocated by `alloc()` with the same `size`.
/// Must not be used after this call.
pub unsafe fn free(ptr: NonNull<u8>, size: usize) {
    let user_size = if size == 0 { 8 } else { size };

    // If the pointer corresponds to a vmalloc block (including small->large fallback),
    // it must be freed via kfree regardless of requested size.
    if let Some(usable) = crate::memory::heap::large::vmalloc::kalloc_usable_size(ptr) {
        crate::memory::heap::large::vmalloc::kfree(ptr, usable);
        HEAP_STATS.large_frees.fetch_add(1, Ordering::Relaxed);
        HEAP_STATS
            .current_inuse
            .fetch_sub(usable as u64, Ordering::Relaxed);
        return;
    }

    if user_size <= HEAP_LARGE_THRESHOLD {
        let sc_entry = match heap_size_class_for(user_size) {
            Some(i) => i,
            None => return,
        };
        let slab_idx = HEAP_SIZE_CLASSES[sc_entry].slab_idx;
        SLUB_CACHES[slab_idx].free(ptr);
        HEAP_STATS.small_frees.fetch_add(1, Ordering::Relaxed);
        // Decrement simply; stats may briefly underestimate under
        // contention but never cause incorrect behavior.
        HEAP_STATS
            .current_inuse
            .fetch_sub(user_size as u64, Ordering::Relaxed);
    } else {
        crate::memory::heap::large::vmalloc::kfree(ptr, user_size);
        HEAP_STATS.large_frees.fetch_add(1, Ordering::Relaxed);
        HEAP_STATS
            .current_inuse
            .fetch_sub(user_size as u64, Ordering::Relaxed);
    }
}
