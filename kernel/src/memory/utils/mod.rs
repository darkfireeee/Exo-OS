// kernel/src/memory/utils/mod.rs
//
// Module utils — futex table (UNIQUE), OOM killer, shrinker.

pub mod futex_table;
pub mod oom_killer;
pub mod shrinker;

// Re-exports futex_table
pub use futex_table::{
    WakeFn, FutexWaiter, FutexBucket, FutexStats, FUTEX_STATS,
    FutexHashTable, FUTEX_TABLE,
    FutexWaitResult, futex_wait, futex_cancel, futex_wake, futex_wake_n, futex_requeue,
};

// Re-exports oom_killer
pub use oom_killer::{
    OomKillCandidate, OomScorer, DefaultOomScorer,
    OomKillSendFn, register_oom_kill_sender,
    OomStats, OOM_STATS,
    select_oom_victim, oom_kill, oom_kill_default,
    oom_suppress, oom_unsuppress,
    set_tsc_hz,
};

// Re-exports shrinker
pub use shrinker::{
    ShrinkerFn, MAX_SHRINKERS, ShrinkerEntry, ShrinkerId,
    ShrinkerStats, SHRINKER_STATS,
    register_shrinker, unregister_shrinker,
    run_shrinkers, shrink_all,
};

/// Initialise tous les utilitaires mémoire.
pub fn init() {
    futex_table::init();
    oom_killer::init();
    shrinker::init();
}
