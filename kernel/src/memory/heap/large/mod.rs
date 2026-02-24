// kernel/src/memory/heap/large/mod.rs
//
// Module "large" : allocateur vmalloc pour allocations > 2048 octets.

pub mod vmalloc;

pub use vmalloc::{kalloc, kfree, krealloc, kalloc_usable_size, VMALLOC_STATS};
