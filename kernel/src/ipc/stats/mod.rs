// ipc/stats/mod.rs — IPC statistics module

pub mod counters;

pub use counters::{IpcStats, IpcStatsSnapshot, StatEvent, IPC_STATS, STAT_COUNT};
