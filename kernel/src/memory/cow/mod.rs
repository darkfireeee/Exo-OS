// kernel/src/memory/cow/mod.rs
//
// Module CoW — Copy-on-Write.

pub mod tracker;
pub mod breaker;

pub use tracker::{CowTracker, COW_TRACKER, COW_TABLE_SIZE};
pub use breaker::{
    CowBreakOutcome, CowBreakerStats, COW_BREAKER_STATS,
    break_cow, try_break_cow,
};
