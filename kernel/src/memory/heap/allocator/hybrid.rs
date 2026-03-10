// kernel/src/memory/heap/allocator/hybrid.rs
//
// Allocateur heap hybride — dispatch entre SLUB (<= 2 KiB) et large (> 2 KiB).
// Interface unique pour le heap kernel.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::memory::core::{AllocError, AllocFlags};
use crate::memory::physical::allocator::slub::{SLUB_CACHES};
use super::size_classes::{heap_size_class_for, HEAP_LARGE_THRESHOLD, HEAP_SIZE_CLASSES};

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES
// ─────────────────────────────────────────────────────────────────────────────

pub struct HybridAllocStats {
    pub small_allocs:  AtomicU64,
    pub small_frees:   AtomicU64,
    pub large_allocs:  AtomicU64,
    pub large_frees:   AtomicU64,
    pub oom_count:     AtomicU64,
    pub current_inuse: AtomicU64,
}

impl HybridAllocStats {
    pub const fn new() -> Self {
        HybridAllocStats {
            small_allocs:  AtomicU64::new(0),
            small_frees:   AtomicU64::new(0),
            large_allocs:  AtomicU64::new(0),
            large_frees:   AtomicU64::new(0),
            oom_count:     AtomicU64::new(0),
            current_inuse: AtomicU64::new(0),
        }
    }
}

pub static HEAP_STATS: HybridAllocStats = HybridAllocStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// ALLOCATEUR HYBRIDE
// ─────────────────────────────────────────────────────────────────────────────

/// Mode d'allocateur utilisé (SLUB ou Large).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocBackend {
    Slub,
    Large,
}

static HYBRID_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    HYBRID_ENABLED.store(true, Ordering::Release);
}

/// Alloue `size` octets avec `align` minimum.
///
/// - Si size <= 2048 : utilise SLUB.
/// - Sinon : utilise le large allocator (vmalloc).
pub fn alloc(size: usize, _align: usize, flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
    if !HYBRID_ENABLED.load(Ordering::Acquire) {
        return Err(AllocError::NotInitialized);
    }

    let real_size = if size == 0 { 8 } else { size };

    if real_size <= HEAP_LARGE_THRESHOLD {
        // Petite allocation via SLUB
        let sc_entry = heap_size_class_for(real_size).ok_or(AllocError::InvalidParams)?;
        let slab_idx = HEAP_SIZE_CLASSES[sc_entry].slab_idx;
        match SLUB_CACHES[slab_idx].alloc(flags) {
            Ok(ptr) => {
                HEAP_STATS.small_allocs.fetch_add(1, Ordering::Relaxed);
                HEAP_STATS.current_inuse.fetch_add(real_size as u64, Ordering::Relaxed);
                Ok(ptr)
            }
            Err(e) => {
                HEAP_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    } else {
        // Grande allocation via le large allocator
        match crate::memory::heap::large::vmalloc::kalloc(real_size, flags) {
            Ok(ptr) => {
                HEAP_STATS.large_allocs.fetch_add(1, Ordering::Relaxed);
                HEAP_STATS.current_inuse.fetch_add(real_size as u64, Ordering::Relaxed);
                Ok(ptr)
            }
            Err(e) => {
                HEAP_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }
}

/// Libère un pointeur alloué par `alloc`.
///
/// SAFETY: `ptr` doit avoir été alloué par `alloc()` avec la même `size`.
///         Ne plus être utilisé après cet appel.
pub unsafe fn free(ptr: NonNull<u8>, size: usize) {
    let real_size = if size == 0 { 8 } else { size };

    if real_size <= HEAP_LARGE_THRESHOLD {
        let sc_entry = match heap_size_class_for(real_size) {
            Some(i) => i,
            None    => return,
        };
        let slab_idx = HEAP_SIZE_CLASSES[sc_entry].slab_idx;
        SLUB_CACHES[slab_idx].free(ptr);
        HEAP_STATS.small_frees.fetch_add(1, Ordering::Relaxed);
        // Décrémenter de façon simple; la stat peut brièvement sous-estimer sous
        // contention mais jamais provoquer de comportement incorrect.
        HEAP_STATS.current_inuse.fetch_sub(real_size as u64, Ordering::Relaxed);
    } else {
        crate::memory::heap::large::vmalloc::kfree(ptr, real_size);
        HEAP_STATS.large_frees.fetch_add(1, Ordering::Relaxed);
        HEAP_STATS.current_inuse.fetch_sub(real_size as u64, Ordering::Relaxed);
    }
}
