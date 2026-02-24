// ipc/stats/mod.rs — Module statistiques IPC

pub mod counters;

pub use counters::{
    StatEvent,
    IpcStats,
    IpcStatsSnapshot,
    IPC_STATS,
    STAT_COUNT,
};
