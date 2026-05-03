// kernel/src/memory/cow/mod.rs
//
// Module CoW — Copy-on-Write.

pub mod breaker;
pub mod tracker;

pub use breaker::{break_cow, try_break_cow, CowBreakOutcome, CowBreakerStats, COW_BREAKER_STATS};
pub use tracker::{CowTracker, COW_TABLE_SIZE, COW_TRACKER};

/// Initialise le suivi CoW pendant le boot mémoire.
///
/// La table est statique ; ce hook documente et fige l'ordre d'initialisation
/// après SLUB, avant les sous-systèmes qui peuvent créer des mappings CoW.
#[inline]
pub fn init() {
    let _ = COW_TRACKER
        .tracked_count
        .load(core::sync::atomic::Ordering::Relaxed);
}
