// kernel/src/memory/numa/mod.rs
//
// Compat — implémentation canonique dans physical/numa/.
// COUCHE 0 — aucune dépendance scheduler/process/ipc/fs.

pub mod distance;
pub mod policy;
pub mod migration;

pub use crate::memory::physical::numa::{
    MAX_NUMA_NODES, NUMA_NODE_INVALID,
    NumaNodeStats, NumaPhysRange, NumaNode, NumaNodeTable, NUMA_NODES,
    NumaGlobalStats, NUMA_GLOBAL_STATS,
    NUMA_DISTANCE_LOCAL, NUMA_DISTANCE_REMOTE, NUMA_DISTANCE_FAR, NUMA_DISTANCE_UNREACHABLE,
    NumaDistanceTable, NUMA_DISTANCE,
    numa_distance, numa_same_node, closest_node,
    NumaNodeMask, NumaPolicy,
    get_system_policy, set_system_policy,
    NumaPolicyStats, NUMA_POLICY_STATS,
    select_node, NumaCpuProvider, BspNumaProvider,
    MigrationStats, MIGRATION_STATS,
    MigrationPageTableOps, MigrateResult,
    frame_node, migrate_page, migrate_pages_batch,
};

/// Initialise le sous-système NUMA.
///
/// # Safety : CPL 0.
pub unsafe fn init() {
    crate::memory::physical::numa::init();
}
