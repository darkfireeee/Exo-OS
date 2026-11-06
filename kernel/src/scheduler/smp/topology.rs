// kernel/src/scheduler/smp/topology.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SMP/NUMA topology — detection and access to inter-CPU distances
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::task::CpuId;
use core::sync::atomic::{AtomicU32, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constants --- static limits (.bss tables, no_alloc)
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_CPUS: usize = 256;
pub const MAX_NODES: usize = 16;
/// Sentinel: absent CPU.
pub const CPU_ABSENT: u32 = u32::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Topological data
// ─────────────────────────────────────────────────────────────────────────────

/// Number of logical CPUs detected.
static NR_CPUS: AtomicU32 = AtomicU32::new(1);
/// Number of NUMA nodes detected.
static NR_NODES: AtomicU32 = AtomicU32::new(1);

/// CPU → NUMA node table. Index = cpu_id, value = node_id.
static mut CPU_TO_NODE: [u8; MAX_CPUS] = [0u8; MAX_CPUS];

/// CPU → physical CPU table (HT sibling). `CPU_ABSENT` if no HT.
static mut CPU_SIBLING: [u32; MAX_CPUS] = [CPU_ABSENT; MAX_CPUS];

/// NUMA distance matrix NR_NODES × NR_NODES.
/// distance[node_a][node_b]: inter-node migration cost (1 = local, >1 = distant).
static mut NUMA_DISTANCE: [[u8; MAX_NODES]; MAX_NODES] = [[1u8; MAX_NODES]; MAX_NODES];

// ─────────────────────────────────────────────────────────────────────────────
// Initialization (called from scheduler::init by BSP)
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes SMP topology.
///
/// # Safety
/// Must be called before any other access to topology tables,
/// and before AP activation.
pub unsafe fn init(nr_cpus: usize, nr_nodes: usize) {
    let nr_cpus = nr_cpus.min(MAX_CPUS);
    let nr_nodes = nr_nodes.min(MAX_NODES);
    NR_CPUS.store(nr_cpus as u32, Ordering::Release);
    NR_NODES.store(nr_nodes as u32, Ordering::Release);

    // Local distance = 10 (SLIT standard), distant = 20.
    for a in 0..nr_nodes {
        for b in 0..nr_nodes {
            NUMA_DISTANCE[a][b] = if a == b { 10 } else { 20 };
        }
    }
}

/// Registers NUMA node of a CPU.
///
/// # Safety
/// Must be called during init, before CPU is brought online.
pub unsafe fn set_cpu_node(cpu: CpuId, node: u8) {
    let idx = cpu.0 as usize;
    if idx < MAX_CPUS {
        CPU_TO_NODE[idx] = node;
    }
}

/// Registers sibling (HyperThread) of a CPU.
///
/// # Safety
/// Must be called during init.
pub unsafe fn set_cpu_sibling(cpu: CpuId, sibling: CpuId) {
    let idx = cpu.0 as usize;
    if idx < MAX_CPUS {
        CPU_SIBLING[idx] = sibling.0;
    }
}

/// Defines distance between two NUMA nodes (symmetric).
///
/// # Safety
/// Must be called during init.
pub unsafe fn set_numa_distance(a: usize, b: usize, dist: u8) {
    if a < MAX_NODES && b < MAX_NODES {
        NUMA_DISTANCE[a][b] = dist;
        NUMA_DISTANCE[b][a] = dist;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Accessors (read-only after init)
// ─────────────────────────────────────────────────────────────────────────────

pub fn nr_cpus() -> usize {
    NR_CPUS.load(Ordering::Relaxed) as usize
}
pub fn nr_nodes() -> usize {
    NR_NODES.load(Ordering::Relaxed) as usize
}

/// Returns NUMA node of CPU `cpu`.
pub fn cpu_node(cpu: CpuId) -> u8 {
    let idx = cpu.0 as usize;
    // SAFETY: idx < MAX_CPUS guaranteed by if; CPU_TO_NODE is a static array
    // initialized by topology_init() before any call to cpu_node().
    if idx < MAX_CPUS {
        unsafe { CPU_TO_NODE[idx] }
    } else {
        0
    }
}

/// Returns HT sibling of CPU `cpu`, or `CPU_ABSENT`.
pub fn cpu_sibling(cpu: CpuId) -> u32 {
    let idx = cpu.0 as usize;
    // SAFETY: idx < MAX_CPUS guaranteed by if; CPU_SIBLING is initialized by
    // topology_init() as read-only after boot.
    if idx < MAX_CPUS {
        unsafe { CPU_SIBLING[idx] }
    } else {
        CPU_ABSENT
    }
}

/// Returns NUMA distance between two nodes.
pub fn numa_distance(a: usize, b: usize) -> u8 {
    // SAFETY: a < MAX_NODES && b < MAX_NODES ensures indices are within
    // bounds of the static 2D array NUMA_DISTANCE.
    if a < MAX_NODES && b < MAX_NODES {
        unsafe { NUMA_DISTANCE[a][b] }
    } else {
        u8::MAX
    }
}

/// Returns `true` if CPU `cpu` is on same NUMA node as `reference`.
pub fn same_node(cpu: CpuId, reference: CpuId) -> bool {
    cpu_node(cpu) == cpu_node(reference)
}
