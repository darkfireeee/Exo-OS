// kernel/src/scheduler/stats/mod.rs

pub mod latency;
pub mod per_cpu;

pub use latency::{LatencyHist, IPI_LATENCY, PICKNEXT_LATENCY, SWITCH_LATENCY, WAKEUP_LATENCY};
pub use per_cpu::{
    add_idle_time, add_run_time, inc_context_switches, inc_ticks, stats as cpu_stats, CpuStats,
};
