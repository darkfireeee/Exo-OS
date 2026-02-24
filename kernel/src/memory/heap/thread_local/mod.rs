// kernel/src/memory/heap/thread_local/mod.rs
//
// Module thread_local : cache per-CPU pour les allocations heap.

pub mod magazine;
pub mod cache;
pub mod drain;

pub use magazine::{Magazine, CpuMagazinePair, MAGAZINE_SIZE};
pub use cache::{
    PerCpuCache, PerCpuCacheTable, CPU_CACHES,
    cache_alloc, cache_free, MAX_CPUS, CACHED_SIZE_CLASSES,
};
pub use drain::{
    DrainPolicy, DRAIN_STATS,
    drain_cpu, drain_on_context_switch, drain_on_memory_pressure,
    drain_all_cpus, drain_cpu_class, total_cached_objects,
};
