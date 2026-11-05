// kernel/src/memory/heap/mod.rs
//
// Heap module: complete kernel allocator.
//
// Architecture:
//   allocator/
//     size_classes  — size classes 8->2048 bytes
//     hybrid        — dispatch SLUB (<=2048) or vmalloc (>2048)
//     global        — #[global_allocator] wrapping hybrid
//   thread_local/
//     magazine      — per-CPU magazine pair
//     cache         — per-CPU cache (lock-free hot path)
//     drain         — cache drain upon ctx switch or memory pressure
//   large/
//     vmalloc       — vmalloc allocator for large allocations

pub mod allocator;
pub mod large;
pub mod thread_local;

// ─────────────────────────────────────────────────────────────────────────────
// PUBLIC RE-EXPORTS
// ─────────────────────────────────────────────────────────────────────────────

pub use allocator::global::KERNEL_ALLOCATOR;
pub use allocator::{heap_alloc, heap_free, heap_init, is_heap_ready, HEAP_STATS};
pub use large::vmalloc::VMALLOC_STATS;
pub use thread_local::drain::{drain_all_cpus, drain_on_context_switch, drain_on_memory_pressure};
