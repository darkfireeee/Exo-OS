// kernel/src/memory/heap/allocator/mod.rs
//
// Module allocator heap — dispatch SLUB + large + global allocator.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod global;
pub mod hybrid;
pub mod size_classes;

pub use size_classes::{
    heap_align_for, heap_alloc_size, heap_size_class_for, HeapSizeClass, HEAP_LARGE_THRESHOLD,
    HEAP_SIZE_CLASSES,
};

pub use global::KERNEL_ALLOCATOR;
pub use hybrid::{alloc as heap_alloc, free as heap_free, init as heap_init, HEAP_STATS};
