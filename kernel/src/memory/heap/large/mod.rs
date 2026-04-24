// kernel/src/memory/heap/large/mod.rs
//
// Module "large" : allocateur vmalloc pour allocations > 2048 octets.

pub mod vmalloc;

pub use vmalloc::{kalloc, kalloc_usable_size, kfree, krealloc, VMALLOC_STATS};
