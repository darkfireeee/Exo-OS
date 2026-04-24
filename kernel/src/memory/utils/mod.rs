// kernel/src/memory/utils/mod.rs
//
// Module utils — futex table (UNIQUE), OOM killer, shrinker.

pub mod futex_table;
pub mod oom_killer;
pub mod shrinker;

// Re-exports futex_table
pub use futex_table::{
    futex_cancel, futex_requeue, futex_wait, futex_wake, futex_wake_n, FutexBucket, FutexHashTable,
    FutexStats, FutexWaitResult, FutexWaiter, WakeFn, FUTEX_STATS, FUTEX_TABLE,
};

// Re-exports oom_killer
pub use oom_killer::{
    oom_kill, oom_kill_default, oom_suppress, oom_unsuppress, register_oom_kill_sender,
    select_oom_victim, set_tsc_hz, DefaultOomScorer, OomKillCandidate, OomKillSendFn, OomScorer,
    OomStats, OOM_STATS,
};

// Re-exports shrinker
pub use shrinker::{
    register_shrinker, run_shrinkers, shrink_all, unregister_shrinker, ShrinkerEntry, ShrinkerFn,
    ShrinkerId, ShrinkerStats, MAX_SHRINKERS, SHRINKER_STATS,
};

/// Initialise tous les utilitaires mémoire.
pub fn init() {
    futex_table::init();
    oom_killer::init();
    shrinker::init();
}
