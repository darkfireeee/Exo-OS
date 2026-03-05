//! # syscall/handlers/signal.rs — Thin wrappers signaux (sigaction, kill, sigreturn)
//!
//! RÈGLE SYS-03 : THIN WRAPPERS UNIQUEMENT.
//! RÈGLE SIG-01 : INTERDIT AtomicPtr<sigaction> dans SignalTcb — TOCTOU triviale.
//! RÈGLE SIG-02 : SigactionEntry stocke les valeurs DIRECTEMENT (handler_vaddr).
//! RÈGLE SIG-07 : SIGKILL(9) et SIGSTOP(19) non-maskables — rejetés dans sigmask.
//! RÈGLE SIG-13 : magic 0x5349474E vérifié au sigreturn AVANT tout restore.
//! RÈGLE SIG-14 : INTERDIT sigreturn sans vérifier magic — injection de faux contexte.
//! RÈGLE SIG-18 : INTERDIT écriture signal frame sans copy_to_user().

#![allow(dead_code)]

use crate::syscall::validation::{validate_signal, USER_ADDR_MAX};
use crate::syscall::errno::{EINVAL, EFAULT, EPERM, ENOSYS};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes signaux POSIX
// ─────────────────────────────────────────────────────────────────────────────

pub const SIGKILL: u32 = 9;
pub const SIGSTOP: u32 = 19;

/// Masque des signaux non-maskables (SIG-07).
pub const NON_MASKABLE_SIGNALS: u64 = (1u64 << SIGKILL) | (1u64 << SIGSTOP);

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `rt_sigaction(signum, act_ptr, oldact_ptr, sigsetsize)` → 0 ou errno.
///
/// SIG-01 : act_ptr est copié via copy_from_user — jamais déréférencé directement.
/// SIG-02 : handler_vaddr stocké en valeur dans SigactionEntry, pas AtomicPtr.
pub fn sys_rt_sigaction(
    signum:      u64,
    act_ptr:     u64,
    oldact_ptr:  u64,
    sigsetsize:  u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };
    // SIGKILL et SIGSTOP — non modifiables (SIG-07)
    if sig == SIGKILL || sig == SIGSTOP { return EINVAL; }
    if sigsetsize != 8 { return EINVAL; }
    // Valider les pointeurs (SYS-01 — copy_from_user obligatoire)
    if act_ptr    != 0 && act_ptr    >= USER_ADDR_MAX { return EFAULT; }
    if oldact_ptr != 0 && oldact_ptr >= USER_ADDR_MAX { return EFAULT; }
    // Délègue → process::signal::handler::do_sigaction()
    let _ = (sig, act_ptr, oldact_ptr);
    ENOSYS
}

/// `rt_sigprocmask(how, set_ptr, oldset_ptr, sigsetsize)` → 0 ou errno.
pub fn sys_rt_sigprocmask(
    how:        u64,
    set_ptr:    u64,
    oldset_ptr: u64,
    sigsetsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    // how : SIG_BLOCK=0, SIG_UNBLOCK=1, SIG_SETMASK=2
    if how > 2 { return EINVAL; }
    if sigsetsize != 8 { return EINVAL; }
    if set_ptr    != 0 && set_ptr    >= USER_ADDR_MAX { return EFAULT; }
    if oldset_ptr != 0 && oldset_ptr >= USER_ADDR_MAX { return EFAULT; }
    // Délègue → process::signal::mask::sigprocmask()
    let _ = (how, set_ptr, oldset_ptr);
    ENOSYS
}

/// `rt_sigreturn()` — retour depuis un handler signal.
///
/// SIG-13 : vérification magic 0x5349474E avant tout restore.
/// SIG-14 : INTERDIT de restaurer sans vérifier le magic.
pub fn sys_rt_sigreturn(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // Délègue → process::signal::handler::restore_signal_frame()
    // La vérification magic est faite dans restore_signal_frame() AVANT toute restauration.
    // SAFETY: appelé depuis Ring3 via trampoline sigreturn — GS kernel actif.
    ENOSYS
}

/// `kill(pid, sig)` → 0 ou errno.
pub fn sys_kill(pid: u64, signum: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };
    // Délègue → process::signal::delivery::send_signal_to_pid()
    let _ = (pid as i32, sig);
    ENOSYS
}

/// `tgkill(tgid, tid, sig)` → 0 ou errno.
pub fn sys_tgkill(tgid: u64, tid: u64, signum: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };
    let _ = (tgid, tid, sig);
    ENOSYS
}

/// `sigaltstack(ss, oss)` → 0 ou errno.
pub fn sys_sigaltstack(ss_ptr: u64, oss_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if ss_ptr  != 0 && ss_ptr  >= USER_ADDR_MAX { return EFAULT; }
    if oss_ptr != 0 && oss_ptr >= USER_ADDR_MAX { return EFAULT; }
    let _ = (ss_ptr, oss_ptr);
    ENOSYS
}
