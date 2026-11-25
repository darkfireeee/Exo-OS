//! # memory/utils/futex_table.rs — Canonical Futex Facade (`DOC3 RULE SCHED-03`)
//!
//! Preserves the canonical `memory::utils::futex_table` module path while
//! delegating implementation to `memory::utils::futex::{bucket_table, wait_wake, syscall_ops}`.

pub use super::futex::{
    futex_cancel, futex_requeue, futex_wait, futex_wake, futex_wake_in_space,
    futex_wake_in_space_bitset, futex_wake_n, init, init_futex_seed, register_sleep_hook,
    register_tid_hook, register_wake_hook, sys_futex, sys_futex_with_timeout,
    wake_clear_child_tid, FutexBucket, FutexError, FutexHashTable, FutexSleepHook, FutexStats,
    FutexTidHook, FutexWaitResult, FutexWaiter, WakeFn, FUTEX_BITSET_MATCH_ANY, FUTEX_STATS,
    FUTEX_TABLE,
};
