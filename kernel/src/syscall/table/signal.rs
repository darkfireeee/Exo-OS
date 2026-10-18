//! # syscall/table/signal.rs — Handlers des signaux POSIX (kill, sigaction, sigaltstack)

use super::common::stat_inc;
use crate::syscall::numbers::*;

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Signaux (délégués vers process/signal/)
// ─────────────────────────────────────────────────────────────────────────────

/// `kill(pid, sig)` — envoie le signal `sig` au processus `pid`.
///
/// ## RÈGLE SIGNAL-01 (DOC1)
/// kill() soumet le signal via process::signal::delivery.
/// La livraison effective se fait au retour userspace du thread cible.
pub fn sys_kill(pid: u64, sig: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_KILL);
    crate::syscall::handlers::signal::sys_kill(pid, sig, 0, 0, 0, 0)
}

/// `tgkill(tgid, tid, sig)` — câblé via send_signal_to_tcb lors de l'intégration.
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

/// `sigaltstack(ss_ptr, old_ss_ptr)` — configure le stack alternatif pour les signaux.
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

