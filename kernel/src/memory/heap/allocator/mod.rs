// kernel/src/memory/heap/allocator/mod.rs
//
// Module allocator heap — dispatch SLUB + large + global allocator.
// Couche 0 — aucune dépendance externe sauf `spin`.

pub mod global;
pub mod hybrid;
pub mod size_classes;

pub use size_classes::{
    HeapSizeClass, HEAP_SIZE_CLASSES, HEAP_LARGE_THRESHOLD,
    heap_size_class_for, heap_alloc_size, heap_align_for,
};

pub use hybrid::{alloc as heap_alloc, free as heap_free, init as heap_init, HEAP_STATS};
pub use global::KERNEL_ALLOCATOR;
