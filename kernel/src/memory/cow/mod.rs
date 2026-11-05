// kernel/src/memory/cow/mod.rs
//
// CoW module — Copy-on-Write.

pub mod breaker;
pub mod tracker;

pub use breaker::{break_cow, try_break_cow, CowBreakOutcome, CowBreakerStats, COW_BREAKER_STATS};
pub use tracker::{CowTracker, COW_TABLE_SIZE, COW_TRACKER};

/// Initializes CoW tracking during memory boot.
///
/// The table is static; this hook documents and freezes the initialization order
/// after SLUB, before subsystems that can create CoW mappings.
#[inline]
pub fn init() {
    let _ = COW_TRACKER
        .tracked_count
        .load(core::sync::atomic::Ordering::Relaxed);
}
