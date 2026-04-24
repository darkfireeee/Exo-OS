// ipc/stats/mod.rs — Module statistiques IPC

pub mod counters;

pub use counters::{IpcStats, IpcStatsSnapshot, StatEvent, IPC_STATS, STAT_COUNT};
