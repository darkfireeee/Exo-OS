// kernel/src/memory/heap/allocator/mod.rs
//
// Heap allocator module — SLUB + large + global allocator dispatch.
// Layer 0 — no external dependencies except `spin`.

pub mod global;
pub mod hybrid;
pub mod size_classes;

pub use size_classes::{
    heap_align_for, heap_alloc_size, heap_size_class_for, HeapSizeClass, HEAP_LARGE_THRESHOLD,
    HEAP_SIZE_CLASSES,
};

pub use global::KERNEL_ALLOCATOR;
pub use hybrid::{
    alloc as heap_alloc, free as heap_free, init as heap_init, is_heap_ready, HEAP_STATS,
};
