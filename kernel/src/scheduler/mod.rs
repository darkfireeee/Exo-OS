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
pub use self::core::{SCHEDULER, init, start, SchedulerStats, yield_now, block_current, unblock};
pub use thread::{Thread, ThreadId, ThreadState, ThreadPriority, ThreadContext};
