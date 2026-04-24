// kernel/src/memory/heap/thread_local/mod.rs
//
// Module thread_local : cache per-CPU pour les allocations heap.

pub mod cache;
pub mod drain;
pub mod magazine;

pub use cache::{
    cache_alloc, cache_free, PerCpuCache, PerCpuCacheTable, CACHED_SIZE_CLASSES, CPU_CACHES,
    MAX_CPUS,
};
pub use drain::{
    drain_all_cpus, drain_cpu, drain_cpu_class, drain_on_context_switch, drain_on_memory_pressure,
    total_cached_objects, DrainPolicy, DRAIN_STATS,
};
pub use magazine::{CpuMagazinePair, Magazine, MAGAZINE_SIZE};
