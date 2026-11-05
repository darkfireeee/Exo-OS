// kernel/src/memory/heap/thread_local/drain.rs
//
// Drain per-CPU caches — triggered upon context switch or
// memory pressure. Returns all cached objects to SLUB pools.
//
// LAYER 0 — no external dependencies.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::heap::allocator::size_classes::HEAP_SIZE_CLASSES;
use crate::memory::heap::thread_local::cache::{CACHED_SIZE_CLASSES, CPU_CACHES, MAX_CPUS};
use crate::memory::physical::allocator::slub::SLUB_CACHES;

// ─────────────────────────────────────────────────────────────────────────────
// DRAIN STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct DrainStats {
    /// Total number of drain operations triggered.
    pub total_drains: AtomicU64,
    /// Number of objects returned to SLUB in total.
    pub objects_drained: AtomicU64,
    /// Drains triggered by memory pressure (OOM shrink).
    pub pressure_drains: AtomicU64,
    /// Drains triggered by context switch.
    pub context_switch_drains: AtomicU64,
    /// Drains triggered explicitly (shutdown / quiesce).
    pub explicit_drains: AtomicU64,
}

impl DrainStats {
    const fn new() -> Self {
        DrainStats {
            total_drains: AtomicU64::new(0),
            objects_drained: AtomicU64::new(0),
            pressure_drains: AtomicU64::new(0),
            context_switch_drains: AtomicU64::new(0),
            explicit_drains: AtomicU64::new(0),
        }
    }
}

pub static DRAIN_STATS: DrainStats = DrainStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// DRAIN POLICY
// ─────────────────────────────────────────────────────────────────────────────

/// Policy determining how many objects to drain per class.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum DrainPolicy {
    /// Full drain: empties both magazines.
    Full,
    /// Partial drain: empties only the `prev` magazine (`loaded` remains intact).
    Partial,
    /// Emergency drain: empties all known CPUs (requires global access).
    AllCpus,
}

// ─────────────────────────────────────────────────────────────────────────────
// DRAIN FUNCTIONS
// ─────────────────────────────────────────────────────────────────────────────

/// Counts the number of drained objects from a cache for a given class.
#[inline]
fn drain_class_inner(cpu_id: usize, class_idx: usize, policy: DrainPolicy) -> usize {
    // SAFETY: cpu_id corresponds to current CPU; no preemption during drain.
    let cache = unsafe { CPU_CACHES.get_mut(cpu_id) };
    let slub_idx = HEAP_SIZE_CLASSES[class_idx].slab_idx;
    let mut count = 0usize;

    match policy {
        DrainPolicy::Full | DrainPolicy::AllCpus => {
            while let Some(ptr) = cache.magazines[class_idx].loaded.pop() {
                // SAFETY: ptr was allocated by SLUB_CACHES[slub_idx].
                unsafe {
                    SLUB_CACHES[slub_idx].free(ptr);
                }
                count += 1;
            }
            while let Some(ptr) = cache.magazines[class_idx].prev.pop() {
                // SAFETY: ptr allocated by SLUB_CACHES[slub_idx].
                // FIX-HEAP-DRAIN-01: the free call remained commented out,
                // causing memory leak of all prev objects during drain.
                unsafe {
                    SLUB_CACHES[slub_idx].free(ptr);
                }
                count += 1;
            }
        }
        DrainPolicy::Partial => {
            while let Some(ptr) = cache.magazines[class_idx].prev.pop() {
                // SAFETY: ptr allocated by SLUB_CACHES[slub_idx] (partial drain).
                unsafe {
                    SLUB_CACHES[slub_idx].free(ptr);
                }
                count += 1;
            }
        }
    }
    count
}

/// Drains per-CPU cache `cpu_id` according to specified policy.
///
/// # Safety
/// Must be called from CPU `cpu_id` (or from context where no other
/// thread can access this CPU's cache).
pub unsafe fn drain_cpu(cpu_id: usize, policy: DrainPolicy) {
    if cpu_id >= MAX_CPUS {
        return;
    }

    let mut total_objects = 0u64;

    for class_idx in 0..CACHED_SIZE_CLASSES {
        total_objects += drain_class_inner(cpu_id, class_idx, policy) as u64;
    }

    DRAIN_STATS.total_drains.fetch_add(1, Ordering::Relaxed);
    DRAIN_STATS
        .objects_drained
        .fetch_add(total_objects, Ordering::Relaxed);
}

/// Drain triggered upon context switch.
///
/// # Safety
/// Same conditions as `drain_cpu`.
#[inline]
pub unsafe fn drain_on_context_switch(cpu_id: usize) {
    // Partial drain on context switch to limit latency.
    drain_cpu(cpu_id, DrainPolicy::Partial);
    DRAIN_STATS
        .context_switch_drains
        .fetch_add(1, Ordering::Relaxed);
}

/// Drain triggered by memory pressure (OOM killer shrinker).
///
/// # Safety
/// Same conditions as `drain_cpu`.
#[inline]
pub unsafe fn drain_on_memory_pressure(cpu_id: usize) {
    drain_cpu(cpu_id, DrainPolicy::Full);
    DRAIN_STATS.pressure_drains.fetch_add(1, Ordering::Relaxed);
}

/// Explicit drain of all active CPUs.
/// Used at shutdown or during global quiesce.
///
/// # Safety
/// Must be called when all CPUs are quiescent (no allocations in progress).
pub unsafe fn drain_all_cpus() {
    for cpu_id in 0..MAX_CPUS {
        let cache = CPU_CACHES.get_mut(cpu_id);
        if !cache.active {
            continue;
        }

        let mut total = 0u64;
        for class_idx in 0..CACHED_SIZE_CLASSES {
            total += drain_class_inner(cpu_id, class_idx, DrainPolicy::AllCpus) as u64;
        }
        DRAIN_STATS
            .objects_drained
            .fetch_add(total, Ordering::Relaxed);
    }
    DRAIN_STATS.total_drains.fetch_add(1, Ordering::Relaxed);
    DRAIN_STATS.explicit_drains.fetch_add(1, Ordering::Relaxed);
}

/// Drains a single size class on a given CPU.
/// Useful for targeted drain (e.g. relieve pressure on a single size).
///
/// # Safety
/// Same conditions as `drain_cpu`.
pub unsafe fn drain_cpu_class(cpu_id: usize, class_idx: usize, policy: DrainPolicy) {
    if cpu_id >= MAX_CPUS || class_idx >= CACHED_SIZE_CLASSES {
        return;
    }
    let count = drain_class_inner(cpu_id, class_idx, policy);
    DRAIN_STATS
        .objects_drained
        .fetch_add(count as u64, Ordering::Relaxed);
    DRAIN_STATS.total_drains.fetch_add(1, Ordering::Relaxed);
}

/// Returns total number of cached objects across all active CPUs.
/// Does not require lock (approximate read-only).
pub fn total_cached_objects() -> usize {
    let mut total = 0usize;
    for cpu_id in 0..MAX_CPUS {
        // SAFETY: approximate read; ignore races (non-critical stat).
        let cache = unsafe { CPU_CACHES.get_mut(cpu_id) };
        if cache.active {
            total += cache.total_cached();
        }
    }
    total
}
