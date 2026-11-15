//! # syscall/table/signal.rs — POSIX signal handlers (kill, sigaction, sigaltstack)

use super::common::stat_inc;
use crate::syscall::numbers::*;

// ─────────────────────────────────────────────────────────────────────────────
// Signal handlers (delegated to process/signal/)
// ─────────────────────────────────────────────────────────────────────────────

/// `kill(pid, sig)` — sends signal `sig` to process `pid`.
///
/// ## RULE SIGNAL-01 (DOC1)
/// kill() submits the signal via process::signal::delivery.
/// Actual delivery occurs upon return to userspace of the target thread.
pub fn sys_kill(pid: u64, sig: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_KILL);
    crate::syscall::handlers::signal::sys_kill(pid, sig, 0, 0, 0, 0)
}

/// `tkill(tid, sig)` — sends signal `sig` to thread `tid`.
pub fn sys_tkill(tid: u64, sig: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_TKILL);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid() as u64;
    crate::syscall::handlers::signal::sys_tgkill(caller_pid, tid, sig, 0, 0, 0)
}

/// `tgkill(tgid, tid, sig)` — wired via send_signal_to_tcb during integration.
pub fn sys_tgkill(tgid: u64, tid: u64, sig: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_TGKILL);
    crate::syscall::handlers::signal::sys_tgkill(tgid, tid, sig, 0, 0, 0)
}

/// `rt_sigaction(sig, act_ptr, oldact_ptr, sigsetsize)`.
pub fn sys_rt_sigaction(
    sig: u64,
    act_ptr: u64,
    oldact_ptr: u64,
    size: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_RT_SIGACTION);
    crate::syscall::handlers::signal::sys_rt_sigaction(sig, act_ptr, oldact_ptr, size, 0, 0)
}

/// `rt_sigprocmask(how, set, oldset, sigsetsize)`.
pub fn sys_rt_sigprocmask(
    how: u64,
    set_ptr: u64,
    oldset_ptr: u64,
    size: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_RT_SIGPROCMASK);
    crate::syscall::handlers::signal::sys_rt_sigprocmask(how, set_ptr, oldset_ptr, size, 0, 0)
}

/// `sigaltstack(ss_ptr, old_ss_ptr)` — configures the alternate stack for signals.
pub fn sys_sigaltstack(
    ss_ptr: u64,
    old_ss_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_SIGALTSTACK);
    crate::syscall::handlers::signal::sys_sigaltstack(ss_ptr, old_ss_ptr, 0, 0, 0, 0)
}

