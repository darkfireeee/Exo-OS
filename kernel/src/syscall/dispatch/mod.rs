//! # syscall/dispatch/mod.rs — Dispatch vers les handlers syscall
//!
//! Orchestre le pipeline complet d'un appel système :
//!
//! ```text
//! syscall_rust_handler(frame)
//!        │
//!        ├─ [0] Validation numéro syscall (bornes)
//!        ├─ [1] Fast-path check (getpid, gettid, clock_gettime…)
//!        │       └─ retour direct si traité (~40-150 cycles)
//!        ├─ [2] Compat translation (numéros Linux alternatifs)
//!        ├─ [3] Sélection handler dans table.rs (O(1))
//!        ├─ [4] Exécution handler
//!        └─ [5] Post-dispatch : signal pending check (RÈGLE SIGNAL-01)
//! ```
//!
//! ## Invariants de sécurité
//! - Aucun handler n'est appelé avec un numéro hors `[0, SYSCALL_TABLE_SIZE)`.
//! - Les registres `rdi..r9` ne sont jamais truqués avant le handler.
//! - Le résultat est toujours écrit dans `frame.rax` avant retour.
//!
//! ## RÈGLE CONTRAT UNSAFE (regle_bonus.md)
//! Tout `unsafe {}` est précédé d'un commentaire `// SAFETY:`.

pub mod process_ops;
pub mod signals;
pub mod stats;

pub use process_ops::{copy_userspace_argv, handle_execve_inplace, handle_fork_like_inplace};
pub use signals::{check_and_deliver_signals, handle_sigreturn_inplace};
pub use stats::{dispatch_stats, reset_dispatch_stats, DispatchStats};

use crate::arch::x86_64::cpu::tsc::read_tsc;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::scheduler::core::switch::current_thread_raw;
use crate::security::audit::syscall_audit::{
    audit_syscall_entry, audit_syscall_exit, AuditVerdict,
};
use crate::syscall::compat::linux::translate_linux_nr;
use crate::syscall::fast_path::try_fast_path;
use crate::syscall::numbers::{is_valid_syscall, ENOSYS};
use crate::syscall::table::get_handler;

// ─────────────────────────────────────────────────────────────────────────────
// Point d'entrée principal du dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch l'appel système décrit par `frame`.
///
/// Cette fonction est appelée depuis `arch::x86_64::syscall::syscall_rust_handler`.
///
/// ## Contrat
/// - `frame` est une `SyscallFrame` validée par le stub ASM `syscall_entry_asm`.
/// - GS est le GS kernel (SWAPGS effectué dans le stub).
/// - La préemption est ACTIVE (pas dans un ISR).
#[no_mangle]
pub fn dispatch(frame: &mut SyscallFrame) {
    // ── [0] Instrumentation début ──────────────────────────────────────────
    let tsc_start = read_tsc();
    stats::inc_total();

    // ── [1] Extraction des arguments ───────────────────────────────────────
    let nr = frame.rax;
    let arg1 = frame.rdi;
    let arg2 = frame.rsi;
    let arg3 = frame.rdx;
    let arg4 = frame.r10;
    let arg5 = frame.r8;
    let arg6 = frame.r9;

    // ── [2] Validation du numéro syscall ───────────────────────────────────
    if !is_valid_syscall(nr) {
        stats::inc_enosys();
        frame.rax = ENOSYS as u64;
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [2b] Audit syscall entry (FIX-APP-02 — GAP-02) ────────────────────
    // audit_syscall_entry() évalue les règles d'audit et peut bloquer le syscall
    // avant même son exécution (AuditVerdict::DenyEperm / ::Kill).
    // Ne concerne que les syscalls sensibles définis dans audit/rules.rs.
    // RÈGLE SAU-01 : ne déclenche aucun appel récursif.
    let (caller_pid, caller_tid) = {
        let tcb_ptr = current_thread_raw();
        if tcb_ptr.is_null() {
            (0u32, 0u32)
        } else {
            // SAFETY: current_thread_raw() retourne un pointeur valide ou null.
            // La durée de vie est bornée à ce stack frame (pas de swap scheduler possible ici).
            let tcb = unsafe { &*tcb_ptr };
            (tcb.pid.0, tcb.tid as u32)
        }
    };
    match audit_syscall_entry(nr as u32, caller_pid, caller_tid, 0) {
        AuditVerdict::Allow => {}
        AuditVerdict::DenyEperm => {
            frame.rax = crate::syscall::numbers::EPERM as u64;
            post_dispatch(frame, tsc_start);
            return;
        }
        AuditVerdict::DenyEnosys => {
            frame.rax = ENOSYS as u64;
            post_dispatch(frame, tsc_start);
            return;
        }
        AuditVerdict::Kill => {
            // Thread marqué pour terminaison — signal SIGKILL immédiat (RÈGLE SAU-02).
            if caller_pid != 0 {
                let _ = crate::process::signal::delivery::send_signal_number_to_pid(
                    crate::process::core::pid::Pid(caller_pid),
                    9, // SIGKILL
                );
            }
            frame.rax = crate::syscall::numbers::EPERM as u64;
            post_dispatch(frame, tsc_start);
            return;
        }
    }

    // ── [2c] Zero-Trust verify_syscall (FIX-APP-01 — GAP-01) ───────────────
    // verify_syscall() est câblé ici pour les syscalls non-fast-path.
    if nr != crate::syscall::numbers::SYS_SCHED_YIELD
        && nr != crate::syscall::numbers::SYS_GETPID
        && nr != crate::syscall::numbers::SYS_CLOCK_GETTIME
    {
        let zt_ok = {
            use crate::security::zero_trust::{context_for_caller, verify_syscall};
            let ctx = context_for_caller(caller_pid, caller_tid);
            verify_syscall(&ctx, nr).is_ok()
        };
        if !zt_ok {
            // TIER 3.1 : forwarder le refus syscall au feed NGAV exo_shield.
            crate::security::shield_feed::push_event(
                caller_pid,
                crate::security::shield_feed::event_type::SYSCALL,
                crate::security::shield_feed::severity::HIGH,
                nr as u32,
                0,
                0,
            );
            frame.rax = crate::syscall::numbers::EPERM as u64;
            audit_syscall_exit(caller_tid, crate::syscall::numbers::EPERM as i64);
            post_dispatch(frame, tsc_start);
            return;
        }
    }

    // ── [3] Fast-path  ─────────────────────────────────────────────────────
    // Couvre les syscalls haute fréquence sans allocation ni verrou.
    if let Some(result) = try_fast_path(nr, arg1, arg2, arg3, arg4, arg5, arg6) {
        stats::inc_fast_path();
        frame.rax = result as u64;
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [4] Traduction numéro compat Linux ─────────────────────────────────
    let effective_nr = match translate_linux_nr(nr) {
        Some(translated) => {
            stats::inc_compat();
            translated
        }
        None => nr,
    };

    // ── [5] Cas spécial rt_sigreturn — besoin d'accéder à la frame arch ────
    if effective_nr == crate::syscall::numbers::SYS_RT_SIGRETURN {
        handle_sigreturn_inplace(frame);
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [5b] Cas spécial fork/vfork — besoin de frame.rcx et frame.rsp ─────
    if effective_nr == crate::syscall::numbers::SYS_FORK {
        stats::syscall_trace(b"sys_fork: dispatch\n");
        let result =
            handle_fork_like_inplace(frame, crate::process::lifecycle::fork::ForkFlags::default());
        stats::syscall_trace(b"sys_fork: result\n");
        frame.rax = result as u64;
        post_dispatch_defer_resched(frame, tsc_start);
        stats::syscall_trace(b"sys_fork: post\n");
        return;
    }

    if effective_nr == crate::syscall::numbers::SYS_VFORK {
        let result = handle_fork_like_inplace(
            frame,
            crate::process::lifecycle::fork::ForkFlags(
                crate::process::lifecycle::fork::ForkFlags::VFORK,
            ),
        );
        frame.rax = result as u64;
        post_dispatch_defer_resched(frame, tsc_start);
        return;
    }

    // ── [5c] Cas spécial execve — modifie frame pour sauter au nouveau binaire ──
    if effective_nr == crate::syscall::numbers::SYS_EXECVE {
        handle_execve_inplace(frame);
        // Pas de post_dispatch après execve réussi (nouvelle image). En cas
        // d'échec, le processus continue dans l'ancienne image et doit livrer
        // ses signaux pendants avant le retour userspace.
        if (frame.rax as i64) < 0 {
            post_dispatch(frame, tsc_start);
        }
        return;
    }

    // ── [6] Slow-path : lookup dans la table ───────────────────────────────
    stats::inc_slow_path();
    let handler = get_handler(effective_nr);

    // ── [7] Exécution du handler ───────────────────────────────────────────
    let result = handler(arg1, arg2, arg3, arg4, arg5, arg6);

    // ── [7b] Comptabilisation d'une erreur ENOSYS ──────────────────────────
    if result == ENOSYS {
        stats::inc_enosys();
    }

    // ── [8] Écriture du résultat ───────────────────────────────────────────
    frame.rax = result as u64;

    // ── [8b] Audit syscall exit (FIX-APP-02) ──────────────────────────────
    audit_syscall_exit(caller_tid, result);

    // ── [8c] Audit ExoLedger : log des syscalls critiques (FIX-APP-08 — GAP-08) ──
    {
        use crate::security::audit::logger::{log_event, AuditCategory, AuditOutcome};
        let is_critical_syscall = effective_nr == crate::syscall::numbers::SYS_EXECVE
            || effective_nr == crate::syscall::numbers::SYS_CLONE
            || effective_nr == crate::syscall::numbers::SYS_FORK
            || effective_nr == crate::syscall::numbers::SYS_VFORK;
        if is_critical_syscall {
            let outcome = if result < 0 {
                AuditOutcome::Error
            } else {
                AuditOutcome::Allow
            };
            log_event(
                AuditCategory::Process,
                caller_pid,
                caller_tid,
                0u16,
                effective_nr as u32,
                result as i32,
                outcome,
                [0u8; 8],
            );
        }
    }

    // ── [9] Post-dispatch : signal pending + instrumentation ──────────────
    post_dispatch(frame, tsc_start);
}

// ─────────────────────────────────────────────────────────────────────────────
// Post-dispatch : signal pending + mesure latence
// ─────────────────────────────────────────────────────────────────────────────

/// Actions effectuées après chaque dispatch.
#[inline]
pub fn post_dispatch(frame: &mut SyscallFrame, tsc_start: u64) {
    post_dispatch_inner(frame, tsc_start, true);
}

#[inline]
pub fn post_dispatch_defer_resched(frame: &mut SyscallFrame, tsc_start: u64) {
    post_dispatch_inner(frame, tsc_start, false);
}

#[inline]
fn post_dispatch_inner(frame: &mut SyscallFrame, tsc_start: u64, allow_resched: bool) {
    if (frame.rax as i64) < 0 {
        crate::arch::x86_64::syscall::record_syscall_error();
    }

    // ── Livraison de signaux pending (RÈGLE SIGNAL-01) ────────────────────
    check_and_deliver_signals(frame);

    // ── Préemption demandée au retour syscall ─────────────────────────────
    if allow_resched {
        unsafe {
            let _ = crate::scheduler::core::switch::schedule_current_if_needed();
        }
    }

    // ── Instrumentation latence (échantillon 1/256) ───────────────────────
    stats::sample_latency(tsc_start);
}

// ─────────────────────────────────────────────────────────────────────────────
// Wrappers de test / diagnostic
// ─────────────────────────────────────────────────────────────────────────────

/// Invoque directement un syscall numéro `nr` avec les arguments donnés,
/// sans passer par la frame ASM. Utilisé par les tests internes.
#[cfg(any(test, feature = "syscall_test"))]
pub fn invoke_direct(
    nr: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    arg6: u64,
) -> i64 {
    if !is_valid_syscall(nr) {
        return ENOSYS;
    }
    if let Some(r) = try_fast_path(nr, arg1, arg2, arg3, arg4, arg5, arg6) {
        return r;
    }
    let effective_nr = translate_linux_nr(nr).unwrap_or(nr);
    let handler = get_handler(effective_nr);
    handler(arg1, arg2, arg3, arg4, arg5, arg6)
}
