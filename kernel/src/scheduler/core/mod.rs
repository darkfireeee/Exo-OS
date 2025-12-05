//! Scheduler core module
//!
//! This module provides the fork-safe scheduler with:
//! - Lock-free pending queue (AtomicPtr + CAS)
//! - Atomic statistics counters
//! - Thread limits and zombie cleanup
//!
//! The `scheduler` module is the main production scheduler (V3).
//! The `scheduler_v2` module is a backup/reference implementation.

pub mod scheduler;
pub mod scheduler_v2;
pub mod affinity;
pub mod statistics;
pub mod predictive;

// V3 exports (main scheduler with lock-free pending queue)
pub use scheduler::{
    Scheduler, 
    QueueType, 
    SchedulerStats,       // Legacy stats struct (Copy/Clone)
    AtomicSchedulerStats, // Lock-free atomic stats
    SchedulerError,       // Error enum
    SCHEDULER, 
    init, 
    start, 
    yield_now, 
    block_current, 
    unblock,
    MAX_THREADS,
    MAX_PENDING_THREADS,
    MAX_ZOMBIE_THREADS,
};

// V2 exports (backup fork-safe implementation)
pub use scheduler_v2::{SchedulerV2, SchedulerStatsV2, SCHEDULER_V2, InterruptGuard};

pub use affinity::{CpuMask, ThreadAffinity};
pub use statistics::SCHEDULER_STATS;
pub use predictive::PredictiveScheduler;