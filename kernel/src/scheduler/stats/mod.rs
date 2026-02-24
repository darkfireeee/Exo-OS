// kernel/src/scheduler/stats/mod.rs

pub mod latency;
pub mod per_cpu;

pub use latency::{SWITCH_LATENCY, WAKEUP_LATENCY, PICKNEXT_LATENCY, IPI_LATENCY, LatencyHist};
pub use per_cpu::{CpuStats, stats as cpu_stats, inc_context_switches, add_run_time, add_idle_time, inc_ticks};
