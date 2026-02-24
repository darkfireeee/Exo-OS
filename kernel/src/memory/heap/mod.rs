// kernel/src/memory/heap/mod.rs
//
// Module heap : allocateur kernel complet.
//
// Architecture :
//   allocator/
//     size_classes  — classes de taille 8→2048 octets
//     hybrid        — dispatch SLUB (≤2048) ou vmalloc (>2048)
//     global        — #[global_allocator] wrapping hybrid
//   thread_local/
//     magazine      — paire de magazines per-CPU
//     cache         — cache per-CPU (hot path sans lock)
//     drain         — drain des caches lors d'un ctx switch ou pression mémoire
//   large/
//     vmalloc       — allocateur vmalloc pour grandes allocations

pub mod allocator;
pub mod thread_local;
pub mod large;

// ─────────────────────────────────────────────────────────────────────────────
// RE-EXPORTS PUBLIQUES
// ─────────────────────────────────────────────────────────────────────────────

pub use allocator::{heap_alloc, heap_free, heap_init, HEAP_STATS};
pub use allocator::global::KERNEL_ALLOCATOR;
pub use thread_local::drain::{drain_on_context_switch, drain_on_memory_pressure, drain_all_cpus};
pub use large::vmalloc::{VMALLOC_STATS};
