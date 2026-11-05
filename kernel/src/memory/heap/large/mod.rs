// kernel/src/memory/heap/large/mod.rs
//
// "large" module: vmalloc allocator for allocations > 2048 bytes.

pub mod vmalloc;

pub use vmalloc::{kalloc, kalloc_usable_size, kfree, krealloc, VMALLOC_STATS};
