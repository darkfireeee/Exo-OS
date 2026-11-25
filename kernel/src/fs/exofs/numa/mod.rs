//! ExoFS NUMA — Main module (wiring facade).

mod facade;
pub mod numa_affinity;
pub mod numa_migration;
pub mod numa_placement;
pub mod numa_stats;
pub mod numa_tuning;

pub use facade::{
    numa_init, numa_preferred_node, NumaConfig, NumaModule, NumaModuleState, NUMA,
};
pub use numa_affinity::{
    AffinityMap, AffinityNodeEntry, CpuId, NumaNodeId, AFFINITY_MAP, CPU_NODE_NONE, MAX_CPUS,
    MAX_NUMA_NODES,
};
pub use numa_migration::{
    BlobNodeLocator, MigrationPolicy, MigrationQueue, MigrationResult, MigrationStatus,
    NumaMigration, MIGRATION_QUEUE_MAX, NUMA_MIGRATION,
};
pub use numa_placement::{
    NumaPlacement, PlacementHint, PlacementResult, PlacementStrategy, NUMA_PLACEMENT,
};
pub use numa_stats::{NumaNodeStats, NumaStats, NUMA_STATS};
pub use numa_tuning::{NumaPolicy, PressureZone, TuningEvent, TuningReport, NUMA_POLICY};
