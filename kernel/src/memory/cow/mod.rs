// kernel/src/memory/cow/mod.rs
//
// Module CoW — Copy-on-Write.

pub mod breaker;
pub mod tracker;

pub use breaker::{break_cow, try_break_cow, CowBreakOutcome, CowBreakerStats, COW_BREAKER_STATS};
pub use tracker::{CowTracker, COW_TABLE_SIZE, COW_TRACKER};
