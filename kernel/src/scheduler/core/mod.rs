//! Scheduler core module

pub mod scheduler;
pub mod affinity;
pub mod statistics;
pub mod predictive;

pub use scheduler::{Scheduler, QueueType, SchedulerStats, SCHEDULER, init, start, yield_now, block_current, unblock};
pub use affinity::{CpuMask, ThreadAffinity};
pub use statistics::SCHEDULER_STATS;
pub use predictive::PredictiveScheduler;