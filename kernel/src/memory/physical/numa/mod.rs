// kernel/src/memory/physical/numa/mod.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// NUMA Module — NUMA memory topology, nodes, distances, policy, migration
// ═══════════════════════════════════════════════════════════════════════════════
//
// Sub-modules:
//   node       — node descriptors, NUMA_NODES table, counters
//   distance   — SLIT distance table, closest_node(), numa_distance()
//   policy     — NumaPolicy, NumaNodeMask, per-thread policies
//   migration  — page migration between nodes
//
// Re-exports to memory::numa (via memory/numa.rs which does `pub use crate::memory::physical::numa::*;`).
//
// LAYER 0 — no scheduler/process/ipc/fs dependencies.
// ═══════════════════════════════════════════════════════════════════════════════

pub mod distance;
pub mod migration;
pub mod node;
pub mod policy;

// ─────────────────────────────────────────────────────────────────────────────
// Public re-exports
// ─────────────────────────────────────────────────────────────────────────────

// Global table of active NUMA nodes.
pub use node::{
    NumaGlobalStats, NumaNode, NumaNodeStats, NumaNodeTable, NumaPhysRange, MAX_NUMA_NODES,
    NUMA_GLOBAL_STATS, NUMA_NODES, NUMA_NODE_INVALID,
};

// Inter-node distances and closest node helper.
pub use distance::{
    closest_node, numa_distance, numa_same_node, NumaDistanceTable, NUMA_DISTANCE,
    NUMA_DISTANCE_FAR, NUMA_DISTANCE_LOCAL, NUMA_DISTANCE_REMOTE,
};

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes NUMA topology: nodes + distance table.
///
/// - Registers node 0 (BSP) by default.
/// - Initializes distance table with default SLIT values
///   (local distance = 10, remote = 20).
///
/// # Safety
/// Called exactly once from `memory::init()` (Phase 8), in Ring 0,
/// prior to AP activation.
pub unsafe fn init() {
    distance::init();
}
