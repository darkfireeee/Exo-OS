// kernel/src/scheduler/smp/mod.rs

pub mod affinity;
pub mod load_balance;
pub mod migration;
pub mod topology;

pub use affinity::{cpu_allowed, sanitize_affinity, CpuMask, CpuSet};
pub use load_balance::{balance_cpu, BALANCE_INTERVAL_TICKS};
pub use migration::{drain_pending_migrations, request_migration};
pub use topology::{cpu_node, init as topology_init, nr_cpus, numa_distance, same_node};
