//! Scheduler subsystem
//! 
//! 3-Queue EMA prediction scheduler with windowed context switching
//! Target: 304 cycle context switch

pub mod core;
pub mod thread;
pub mod switch;
pub mod idle;
pub mod prediction;
pub mod realtime;
pub mod test_threads;

// Re-exports
pub use self::core::{SCHEDULER, init, start, SchedulerStats, yield_now, block_current, unblock, run_context_switch_benchmark};
pub use thread::{Thread, ThreadId, ThreadState, ThreadPriority, ThreadContext};

/// Convenient function to get scheduler statistics
pub fn get_stats() -> SchedulerStatsSimple {
    let stats = SCHEDULER.stats();
    SchedulerStatsSimple {
        total_spawns: stats.total_spawns,
        total_switches: stats.total_switches,
        ready_queue_len: stats.hot_queue_len + stats.normal_queue_len + stats.cold_queue_len,
    }
}

/// Simplified scheduler stats for shell display
#[derive(Debug, Clone, Copy)]
pub struct SchedulerStatsSimple {
    pub total_spawns: u64,
    pub total_switches: u64,
    pub ready_queue_len: usize,
}
