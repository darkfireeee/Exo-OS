//! # memory/utils/futex — Modular Global Futex Implementation
//!
//! Exposed through `memory/utils/futex_table.rs` to honor `DOC3 RULE SCHED-03`.
//! - `bucket_table` : SipHash-1-3 keyed 4096-bucket hash table, `FutexWaiter`, `FutexStats`
//! - `wait_wake`    : `futex_wait*`, `futex_cancel`, `futex_wake*`, `futex_requeue*`
//! - `syscall_ops`  : `FutexError`, scheduler sleep/wake/tid hooks, `sys_futex*`

pub(crate) mod bucket_table;
pub(crate) mod syscall_ops;
pub(crate) mod wait_wake;
#[cfg(test)]
mod tests;

pub use bucket_table::{
    init_futex_seed, FutexBucket, FutexHashTable, FutexStats, FutexWaiter, WakeFn,
    FUTEX_BITSET_MATCH_ANY, FUTEX_STATS, FUTEX_TABLE,
};
pub use syscall_ops::{
    register_sleep_hook, register_tid_hook, register_wake_hook, sys_futex, sys_futex_with_timeout,
    FutexError, FutexSleepHook, FutexTidHook,
};
pub use wait_wake::{
    futex_cancel, futex_requeue, futex_wait, futex_wake, futex_wake_in_space,
    futex_wake_in_space_bitset, futex_wake_n, init, wake_clear_child_tid, FutexWaitResult,
};
