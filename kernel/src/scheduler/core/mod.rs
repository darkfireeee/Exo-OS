//! Scheduler core module
//!
//! This module provides two scheduler implementations:
//! - `scheduler`: Original V1 scheduler (has fork deadlock issues)
//! - `scheduler_v2`: New fork-safe scheduler with lock-free pending queue
//!
//! Use SCHEDULER_V2 for fork-safe operations.

pub mod scheduler;
pub mod scheduler_v2;
pub mod affinity;
pub mod statistics;
pub mod predictive;

// V1 exports (for backward compatibility)
pub use scheduler::{Scheduler, QueueType, SchedulerStats, SCHEDULER, init, start, yield_now, block_current, unblock};

// V2 exports (fork-safe)
pub use scheduler_v2::{SchedulerV2, SchedulerStatsV2, SCHEDULER_V2, InterruptGuard};

pub use affinity::{CpuMask, ThreadAffinity};
pub use statistics::SCHEDULER_STATS;
pub use predictive::PredictiveScheduler;