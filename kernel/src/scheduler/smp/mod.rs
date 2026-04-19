// kernel/src/scheduler/smp/mod.rs

pub mod affinity;
pub mod load_balance;
pub mod migration;
pub mod topology;

pub use affinity::{CpuMask, CpuSet, cpu_allowed, sanitize_affinity};
pub use load_balance::{balance_cpu, BALANCE_INTERVAL_TICKS};
pub use migration::{request_migration, drain_pending_migrations};
pub use topology::{nr_cpus, cpu_node, numa_distance, same_node, init as topology_init};
