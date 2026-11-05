// kernel/src/memory/heap/allocator/size_classes.rs
//
// Size classes for the hybrid heap allocator.
// Covers 1 byte up to 2 KiB (beyond -> large allocator).
// Layer 0 — no external dependencies except `spin`.

// ─────────────────────────────────────────────────────────────────────────────
// SIZE CLASSES
// ─────────────────────────────────────────────────────────────────────────────

/// An entry in the size classes table.
#[derive(Debug, Clone, Copy)]
pub struct HeapSizeClass {
    /// Maximum size in bytes for this class.
    pub max_size: usize,
    /// Alignment (always a power of 2).
    pub align: usize,
    /// Index of the corresponding slab/slub cache.
    pub slab_idx: usize,
}

/// Size class table of the hybrid heap.
/// Sizes: 8, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048
pub const HEAP_SIZE_CLASSES: &[HeapSizeClass] = &[
    HeapSizeClass {
        max_size: 8,
        align: 8,
        slab_idx: 0,
    },
    HeapSizeClass {
        max_size: 16,
        align: 16,
        slab_idx: 1,
    },
    HeapSizeClass {
        max_size: 24,
        align: 8,
        slab_idx: 2,
    }, // -> class 32 slab
    HeapSizeClass {
        max_size: 32,
        align: 16,
        slab_idx: 2,
    },
    HeapSizeClass {
        max_size: 48,
        align: 16,
        slab_idx: 3,
    }, // -> class 64 slab
    HeapSizeClass {
        max_size: 64,
        align: 64,
        slab_idx: 3,
    },
    HeapSizeClass {
        max_size: 96,
        align: 64,
        slab_idx: 4,
    }, // -> class 128 slab
    HeapSizeClass {
        max_size: 128,
        align: 64,
        slab_idx: 4,
    },
    HeapSizeClass {
        max_size: 192,
        align: 64,
        slab_idx: 5,
    }, // -> class 256 slab
    HeapSizeClass {
        max_size: 256,
        align: 64,
        slab_idx: 5,
    },
    HeapSizeClass {
        max_size: 384,
        align: 64,
        slab_idx: 6,
    }, // -> class 512 slab
    HeapSizeClass {
        max_size: 512,
        align: 64,
        slab_idx: 6,
    },
    HeapSizeClass {
        max_size: 768,
        align: 64,
        slab_idx: 7,
    }, // -> class 1024 slab
    HeapSizeClass {
        max_size: 1024,
        align: 64,
        slab_idx: 7,
    },
    HeapSizeClass {
        max_size: 1536,
        align: 64,
        slab_idx: 8,
    }, // -> class 2048 slab
    HeapSizeClass {
        max_size: 2048,
        align: 64,
        slab_idx: 8,
    },
];

/// Threshold in bytes above which the large allocator is used.
pub const HEAP_LARGE_THRESHOLD: usize = 2048;

/// Number of available size classes.
pub const NUM_HEAP_SIZE_CLASSES: usize = 16;

/// Returns the size class index for `size` bytes.
/// Returns `None` if `size > HEAP_LARGE_THRESHOLD`.
#[inline]
pub fn heap_size_class_for(size: usize) -> Option<usize> {
    if size == 0 {
        return Some(0);
    } // Allocates 8 bytes minimum
    for (i, sc) in HEAP_SIZE_CLASSES.iter().enumerate() {
        if size <= sc.max_size {
            return Some(i);
        }
    }
    None
}

/// Returns the actual allocated size for `size` (rounded up to size class).
#[inline]
pub fn heap_alloc_size(size: usize) -> usize {
    if size == 0 {
        return HEAP_SIZE_CLASSES[0].max_size;
    }
    for sc in HEAP_SIZE_CLASSES {
        if size <= sc.max_size {
            return sc.max_size;
        }
    }
    // Large allocation: round up to PAGE_SIZE
    (size + crate::memory::core::PAGE_SIZE - 1) & !(crate::memory::core::PAGE_SIZE - 1)
}

/// Returns the required alignment for `size`.
#[inline]
pub fn heap_align_for(size: usize) -> usize {
    for sc in HEAP_SIZE_CLASSES {
        if size <= sc.max_size {
            return sc.align;
        }
    }
    crate::memory::core::PAGE_SIZE
}
