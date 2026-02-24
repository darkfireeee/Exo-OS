// kernel/src/memory/physical/numa/mod.rs
//
// Module NUMA canonique — nœuds NUMA, distances, politiques, migration.
//
// Implémentation conforme à l'arborescence (docs/kernel/memory/arborescence memory.txt).
// COUCHE 0 — aucune dépendance scheduler/process/ipc/fs.

pub mod node;
pub mod distance;
pub mod policy;
pub mod migration;

// ─────────────────────────────────────────────────────────────────────────────
// Re-exports
// ─────────────────────────────────────────────────────────────────────────────

pub use node::{
    MAX_NUMA_NODES, NUMA_NODE_INVALID,
    NumaNodeStats, NumaPhysRange, NumaNode, NumaNodeTable, NUMA_NODES,
    NumaGlobalStats, NUMA_GLOBAL_STATS,
};

pub use distance::{
    NUMA_DISTANCE_LOCAL, NUMA_DISTANCE_REMOTE, NUMA_DISTANCE_FAR, NUMA_DISTANCE_UNREACHABLE,
    NumaDistanceTable, NUMA_DISTANCE,
    numa_distance, numa_same_node, closest_node,
};

// NumaPolicy (placement) est distinct de physical::allocator::NumaPolicy (allocateur).
// Accessible via ce module comme `physical::numa::NumaPolicy`.
pub use policy::{
    NumaNodeMask, NumaPolicy,
    get_system_policy, set_system_policy,
    NumaPolicyStats, NUMA_POLICY_STATS,
    select_node, NumaCpuProvider, BspNumaProvider,
};

pub use migration::{
    MigrationStats, MIGRATION_STATS,
    MigrationPageTableOps, MigrateResult,
    frame_node, migrate_page, migrate_pages_batch,
};

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise l'ensemble du sous-système NUMA.
///
/// # Safety : CPL 0.
pub unsafe fn init() {
    node::init();
    distance::init();
    policy::init();
    migration::init();
}
