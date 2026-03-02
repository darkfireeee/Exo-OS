//! numa/ — Gestion de la topologie NUMA pour ExoFS (no_std).

pub mod numa_affinity;
pub mod numa_migration;
pub mod numa_placement;
pub mod numa_stats;
pub mod numa_tuning;

pub use numa_affinity::{AffinityMap, AFFINITY_MAP};
pub use numa_migration::{NumaMigration, MigrationResult};
pub use numa_placement::{NumaPlacement, NUMA_PLACEMENT};
pub use numa_stats::{NUMA_STATS, NumaNodeStats};
pub use numa_tuning::{NumaPolicy, NUMA_POLICY};
