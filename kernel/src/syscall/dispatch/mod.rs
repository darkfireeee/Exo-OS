//! # syscall/dispatch/mod.rs — Dispatch to syscall handlers
//!
//! Orchestrates the full pipeline of a system call:
//!
//! ```text
//! syscall_rust_handler(frame)
//!        │
//!        ├─ [0] Syscall number validation (bounds)
//!        ├─ [1] Fast-path check (getpid, gettid, clock_gettime…)
//!        │       └─ direct return if handled (~40-150 cycles)
//!        ├─ [2] Compat translation (alternate Linux numbers)
//!        ├─ [3] Handler selection in table.rs (O(1))
//!        ├─ [4] Handler execution
//!        └─ [5] Post-dispatch: signal pending check (RULE SIGNAL-01)
//! ```
//!
//! ## Security Invariants
//! - No handler is called with a number outside `[0, SYSCALL_TABLE_SIZE)`.
//! - Registers `rdi..r9` are never falsified before handler.
//! - Result is always written into `frame.rax` before return.
//!
//! ## UNSAFE CONTRACT RULE (regle_bonus.md)
//! Every `unsafe {}` block is preceded by a `// SAFETY:` comment.

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
// Main dispatch entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatches the system call described by `frame`.
///
/// This function is called from `arch::x86_64::syscall::syscall_rust_handler`.
///
/// ## Contract
/// - `frame` is a `SyscallFrame` validated by ASM stub `syscall_entry_asm`.
/// - GS is the kernel GS (SWAPGS performed in stub).
/// - Preemption is ACTIVE (not in an ISR).
#[no_mangle]
pub fn dispatch(frame: &mut SyscallFrame) {
    // ── [0] Start instrumentation ──────────────────────────────────────────
    let tsc_start = read_tsc();
    stats::inc_total();

    // ── [1] Argument extraction ───────────────────────────────────────────
    let nr = frame.rax;
    let arg1 = frame.rdi;
    let arg2 = frame.rsi;
    let arg3 = frame.rdx;
    let arg4 = frame.r10;
    let arg5 = frame.r8;
    let arg6 = frame.r9;

    // ── [2] Syscall number validation ─────────────────────────────────────
    if !is_valid_syscall(nr) {
        stats::inc_enosys();
        frame.rax = ENOSYS as u64;
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [2b] Audit syscall entry (FIX-APP-02 — GAP-02) ────────────────────
    // audit_syscall_entry() evaluates audit rules and can block the syscall
    // even before its execution (AuditVerdict::DenyEperm / ::Kill).
    // Only affects sensitive syscalls defined in audit/rules.rs.
    // RULE SAU-01: triggers no recursive calls.
    let (caller_pid, caller_tid) = {
        let tcb_ptr = current_thread_raw();
        if tcb_ptr.is_null() {
            (0u32, 0u32)
        } else {
            // SAFETY: current_thread_raw() returns a valid pointer or null.
            // Lifetime is bounded to this stack frame (no scheduler swap possible here).
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
            // Thread marked for termination — immediate SIGKILL signal (RULE SAU-02).
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
    // verify_syscall() is wired here for non-fast-path syscalls.
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
            // TIER 3.1: forward syscall denial to exo_shield NGAV feed.
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
    // Covers high-frequency syscalls without allocation or lock.
    if let Some(result) = try_fast_path(nr, arg1, arg2, arg3, arg4, arg5, arg6) {
        stats::inc_fast_path();
        frame.rax = result as u64;
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [4] Linux compat number translation ────────────────────────────────
    let effective_nr = match translate_linux_nr(nr) {
        Some(translated) => {
            stats::inc_compat();
            translated
        }
        None => nr,
    };

    // ── [5] Special case rt_sigreturn — needs access to arch frame ────────
    if effective_nr == crate::syscall::numbers::SYS_RT_SIGRETURN {
        handle_sigreturn_inplace(frame);
        post_dispatch(frame, tsc_start);
        return;
    }

    // ── [5b] Special case fork/vfork — needs frame.rcx and frame.rsp ───────
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

    if effective_nr == crate::syscall::numbers::SYS_CLONE && (arg1 & 0x0001_0000) == 0 {
        let mut fork_flags = crate::process::lifecycle::fork::ForkFlags::default();
        if (arg1 & 0x0000_0400) != 0 {
            fork_flags = fork_flags.set(crate::process::lifecycle::fork::ForkFlags::CLONE_FILES);
        }
        if (arg1 & 0x0000_0800) != 0 {
            fork_flags = fork_flags.set(crate::process::lifecycle::fork::ForkFlags::CLONE_SIGHAND);
        }
        if (arg1 & 0x0000_4000) != 0 {
            fork_flags = fork_flags.set(crate::process::lifecycle::fork::ForkFlags::VFORK);
        }
        let saved_rsp = frame.rsp;
        if arg2 != 0 {
            frame.rsp = arg2;
        }
        let result = handle_fork_like_inplace(frame, fork_flags);
        frame.rsp = saved_rsp;
        frame.rax = result as u64;
        post_dispatch_defer_resched(frame, tsc_start);
        return;
    }

    // ── [5c] Special case execve — modifies frame to jump to new binary ────
    if effective_nr == crate::syscall::numbers::SYS_EXECVE {
        handle_execve_inplace(frame);
        // No post_dispatch after successful execve (new image). On
        // failure, the process continues in the old image and must deliver
        // its pending signals before returning to userspace.
        if (frame.rax as i64) < 0 {
            post_dispatch(frame, tsc_start);
        }
        return;
    }

    // ── [6] Slow-path: lookup in table ────────────────────────────────────
    stats::inc_slow_path();
    let handler = get_handler(effective_nr);

    // ── [7] Handler execution ─────────────────────────────────────────────
    let result = handler(arg1, arg2, arg3, arg4, arg5, arg6);

    // ── [7b] Accounting for an ENOSYS error ───────────────────────────────
    if result == ENOSYS {
        stats::inc_enosys();
    }

    // ── [8] Writing result ────────────────────────────────────────────────
    frame.rax = result as u64;

    // ── [8b] Audit syscall exit (FIX-APP-02) ──────────────────────────────
    audit_syscall_exit(caller_tid, result);

    // ── [8c] Audit ExoLedger: log critical syscalls (FIX-APP-08 — GAP-08) ──
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

    // ── [9] Post-dispatch: signal pending + instrumentation ───────────────
    post_dispatch(frame, tsc_start);
}

// ─────────────────────────────────────────────────────────────────────────────
// Post-dispatch: signal pending + latency measurement
// ─────────────────────────────────────────────────────────────────────────────

/// Actions performed after each dispatch.
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

    // ── Pending signal delivery (RULE SIGNAL-01) ─────────────────────────
    check_and_deliver_signals(frame);

    // ── Preemption requested upon syscall return ──────────────────────────
    if allow_resched {
        unsafe {
            let _ = crate::scheduler::core::switch::schedule_current_if_needed();
        }
    }

    // ── Latency instrumentation (1/256 sample) ───────────────────────────
    stats::sample_latency(tsc_start);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test / diagnostic wrappers
// ─────────────────────────────────────────────────────────────────────────────

/// Directly invokes syscall number `nr` with given arguments,
/// without going through the ASM frame. Used by internal tests.
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
