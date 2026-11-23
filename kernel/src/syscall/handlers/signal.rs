//! # syscall/handlers/signal.rs — Signal thin wrappers (sigaction, kill, sigreturn)
//!
//! RULE SYS-03: THIN WRAPPERS ONLY.
//! RULE SIG-01: FORBIDDEN AtomicPtr<sigaction> in SignalTcb — trivial TOCTOU.
//! RULE SIG-02: SigactionEntry stores values DIRECTLY (handler_vaddr).
//! RULE SIG-07: SIGKILL(9) and SIGSTOP(19) non-maskable — rejected in sigmask.
//! RULE SIG-13: magic 0x5349474E verified at sigreturn BEFORE any restore.
//! RULE SIG-14: FORBIDDEN sigreturn without verifying magic — injection of fake context.
//! RULE SIG-18: FORBIDDEN writing signal frame without copy_to_user().

use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::process::signal::default::{SigAction, SigActionKind};
use crate::scheduler::core::task::ThreadControlBlock;
use crate::syscall::errno::{EFAULT, EINVAL, ENOSYS, EPERM, ESRCH};
use crate::syscall::validation::{
    read_user_typed, validate_signal, write_user_typed, USER_ADDR_MAX,
};

/// Linux layout of struct sigaction (x86_64) as passed by userspace.
/// Size: 32 bytes — MUST match Linux glibc/musl ABI.
#[derive(Copy, Clone)]
#[repr(C)]
struct LinuxSigaction {
    sa_handler: u64,  // offset  0: SIG_DFL=0, SIG_IGN=1, or handler address
    sa_flags: u64,    // offset  8: SA_RESTART | SA_SIGINFO | SA_ONSTACK...
    sa_restorer: u64, // offset 16: rt_sigreturn stub address
    sa_mask: u64,     // offset 24: mask of signals blocked during handler
}

/// Returns TCB pointer of current thread from gs:[0x20].
/// Returns 0 if no thread is active (boot/idle).
///
/// # Safety
/// Kernel GS must be active (SWAPGS executed in syscall stub).
#[inline]
unsafe fn current_tcb_ptr() -> *const ThreadControlBlock {
    let p: u64;
    core::arch::asm!("mov {}, gs:[0x20]", out(reg) p, options(nostack, nomem));
    p as *const ThreadControlBlock
}

// ─────────────────────────────────────────────────────────────────────────────
// POSIX signal constants
// ─────────────────────────────────────────────────────────────────────────────

pub const SIGKILL: u32 = 9;
pub const SIGSTOP: u32 = 19;

/// Mask of non-maskable signals (SIG-07).
pub const NON_MASKABLE_SIGNALS: u64 = (1u64 << SIGKILL) | (1u64 << SIGSTOP);

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `rt_sigaction(signum, act_ptr, oldact_ptr, sigsetsize)` -> 0 or errno.
///
/// SIG-01: act_ptr is copied via validated userspace read — never dereferenced directly.
/// SIG-02: handler_vaddr stored by value in SigAction, not AtomicPtr.
pub fn sys_rt_sigaction(
    signum: u64,
    act_ptr: u64,
    oldact_ptr: u64,
    sigsetsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };
    // SIGKILL and SIGSTOP — immutable (SIG-07)
    if sig == SIGKILL || sig == SIGSTOP {
        return EINVAL;
    }
    if sigsetsize != 8 {
        return EINVAL;
    }
    // Validate pointers (SYS-01)
    if act_ptr != 0 && act_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if oldact_ptr != 0 && oldact_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }

    // Obtain current process PCB via TCB.
    // SAFETY: kernel GS active, gs:[0x20] = valid TCB pointer or 0.
    let tcb_ptr = unsafe { current_tcb_ptr() };
    if tcb_ptr.is_null() {
        return ENOSYS;
    }
    let pid = unsafe { Pid((*tcb_ptr).pid.0) };
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => return EFAULT,
    };

    let sig_tcb_opt = {
        let vaddr = pcb.signal_tcb_vaddr.load(core::sync::atomic::Ordering::Acquire);
        if (0x1000..USER_ADDR_MAX).contains(&vaddr) {
            crate::syscall::validation::resolve_user_page_kernel_alias(vaddr, act_ptr != 0)
                .ok()
                .map(|alias| unsafe { &*(alias as *const crate::process::signal::tcb::SignalTcb) })
        } else {
            None
        }
    };

    // Read old action and write it to oldact_ptr if requested.
    if oldact_ptr != 0 {
        let mut old_action = {
            let handlers = pcb.sig_handlers.lock();
            handlers.get(sig as u8)
        };
        if let Some(stcb) = sig_tcb_opt {
            if let Some(entry) = stcb.atomic_get_action(sig as u8) {
                old_action = SigAction {
                    kind: if entry.is_dfl() {
                        SigActionKind::Term
                    } else if entry.is_ign() {
                        SigActionKind::Ignore
                    } else {
                        SigActionKind::User
                    },
                    handler: entry.handler_vaddr,
                    flags: entry.flags,
                    mask: entry.mask,
                    restorer: entry.restorer,
                };
            }
        }
        // Construct LinuxSigaction from SigAction (for userspace ABI).
        let linux_old = LinuxSigaction {
            sa_handler: if old_action.kind == SigActionKind::Ignore {
                1
            } else if old_action.kind == SigActionKind::User {
                old_action.handler
            } else {
                0
            },
            sa_flags: old_action.flags as u64,
            sa_restorer: old_action.restorer,
            sa_mask: old_action.mask,
        };
        if write_user_typed(oldact_ptr, linux_old).is_err() {
            return EFAULT;
        }
    }

    // Read and install new action if act_ptr is provided.
    if act_ptr != 0 {
        let linux_act = match read_user_typed::<LinuxSigaction>(act_ptr) {
            Ok(act) => act,
            Err(_) => return EFAULT,
        };

        let kind = if linux_act.sa_handler == 0 {
            SigActionKind::Term // SIG_DFL — actual action depends on signal, Term by default
        } else if linux_act.sa_handler == 1 {
            SigActionKind::Ignore // SIG_IGN
        } else {
            SigActionKind::User
        };

        let new_action = SigAction {
            kind,
            handler: linux_act.sa_handler,
            flags: linux_act.sa_flags as u32,
            mask: linux_act.sa_mask,
            restorer: linux_act.sa_restorer,
        };

        {
            let mut handlers = pcb.sig_handlers.lock();
            handlers.set(sig as u8, new_action);
        }
        if let Some(stcb) = sig_tcb_opt {
            let entry = crate::process::signal::tcb::SigactionEntry {
                handler_vaddr: linux_act.sa_handler,
                flags: linux_act.sa_flags as u32,
                _pad: 0,
                mask: linux_act.sa_mask,
                restorer: linux_act.sa_restorer,
            };
            let _ = stcb.atomic_set_action(sig as u8, entry);
        }
    }

    0 // success
}

/// `rt_sigprocmask(how, set_ptr, oldset_ptr, sigsetsize)` -> 0 or errno.
pub fn sys_rt_sigprocmask(
    how: u64,
    set_ptr: u64,
    oldset_ptr: u64,
    sigsetsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    // how: SIG_BLOCK=0, SIG_UNBLOCK=1, SIG_SETMASK=2
    if how > 2 {
        return EINVAL;
    }
    if sigsetsize != 8 {
        return EINVAL;
    }
    if set_ptr != 0 && set_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if oldset_ptr != 0 && oldset_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }

    // Obtain TCB of current thread.
    // SAFETY: kernel GS active in syscall context.
    let tcb_ptr = unsafe { current_tcb_ptr() };
    if tcb_ptr.is_null() {
        return ENOSYS;
    }
    let tcb = unsafe { &*tcb_ptr };

    // Read old mask.
    let old_mask = tcb.signal_mask.load(core::sync::atomic::Ordering::Acquire);

    // Write oldset if requested.
    if oldset_ptr != 0 {
        if write_user_typed(oldset_ptr, old_mask).is_err() {
            return EFAULT;
        }
    }

    // Apply new mask if set_ptr is provided.
    if set_ptr != 0 {
        let new_set = match read_user_typed::<u64>(set_ptr) {
            Ok(set) => set,
            Err(_) => return EFAULT,
        };

        let computed = match how {
            0 => old_mask | new_set,  // SIG_BLOCK
            1 => old_mask & !new_set, // SIG_UNBLOCK
            2 => new_set,             // SIG_SETMASK
            _ => return EINVAL,
        };
        // Force SIGKILL/SIGSTOP unblockable and remove invalid bit 64.
        let safe_mask = crate::process::signal::mask::SigMask::from(computed).0;
        tcb.signal_mask
            .store(safe_mask, core::sync::atomic::Ordering::Release);

        if let Some(pcb) = PROCESS_REGISTRY.find_by_pid(Pid(tcb.pid.0)) {
            let tid = crate::process::core::pid::Tid(tcb.tid as u32);
            let mut thread_ptr = pcb.find_thread_by_tid(tid);
            if thread_ptr.is_null() {
                thread_ptr = pcb.find_alive_thread();
            }
            if !thread_ptr.is_null() {
                let thread = unsafe { &*thread_ptr };
                crate::process::signal::delivery::sync_pending_after_unblock(thread, safe_mask);
            }
            if pcb.thread_count.load(core::sync::atomic::Ordering::Acquire) <= 1 {
                let vaddr = pcb
                    .signal_tcb_vaddr
                    .load(core::sync::atomic::Ordering::Acquire);
                if (0x1000..USER_ADDR_MAX).contains(&vaddr) {
                    if let Ok(alias) =
                        crate::syscall::validation::resolve_user_page_kernel_alias(vaddr, true)
                    {
                        let stcb =
                            unsafe { &*(alias as *const crate::process::signal::tcb::SignalTcb) };
                        stcb.set_mask(safe_mask);
                    }
                }
            }
        }
    }

    0 // success
}

/// `rt_sigreturn()` — return from a signal handler.
///
/// SIG-13: verify magic 0x5349474E before any restore.
/// SIG-14: FORBIDDEN to restore without verifying magic.
pub fn sys_rt_sigreturn(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // NOTE: this table handler is not the actual path. `dispatch.rs`
    // intercepts SYS_RT_SIGRETURN with the arch frame and calls
    // handle_sigreturn_inplace(), the only place where registers can be
    // restored correctly. If this handler is reached, the table has been
    // reorganized inconsistently.
    crate::arch::x86_64::terminal::debug_write(b"rt_sigreturn: handler table atteint\n");
    EINVAL
}

/// `kill(pid, sig)` -> 0 or errno.
pub fn sys_kill(pid: u64, signum: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::process::signal::delivery::{send_signal_number_to_pid, SendError};

    let signed_pid = pid as i64;
    if signed_pid > i32::MAX as i64 || signed_pid < i32::MIN as i64 {
        return ESRCH;
    }
    let target_pid = signed_pid as i32;
    let real_pid: u32 = if target_pid <= 0 {
        // pid==0: send to current group -> approx. with current PID.
        // pid<0: process groups — not implemented.
        if target_pid == 0 {
            unsafe {
                let ptr: u64;
                core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr,
                    options(nostack, nomem));
                if ptr == 0 {
                    return EFAULT;
                }
                (*(ptr as *const ThreadControlBlock)).pid.0
            }
        } else {
            return ESRCH;
        }
    } else {
        target_pid as u32
    };

    // Verify POSIX privileges for kill (AUDIT-SEC3)
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid != real_pid {
        let caller_pcb = match PROCESS_REGISTRY.find_by_pid(Pid(caller_pid)) {
            Some(p) => p,
            None => return EPERM,
        };
        let target_pcb = match PROCESS_REGISTRY.find_by_pid(Pid(real_pid)) {
            Some(p) => p,
            None => return ESRCH,
        };
        let c_creds = caller_pcb.get_creds();
        let t_creds = target_pcb.get_creds();
        let is_privileged = c_creds.is_root() || c_creds.euid == 0;
        let uid_match = c_creds.uid == t_creds.uid
            || c_creds.euid == t_creds.uid
            || c_creds.euid == t_creds.euid;
        if !is_privileged && !uid_match {
            return EPERM;
        }
    }

    if signum == 0 {
        let found = PROCESS_REGISTRY.find_by_pid(Pid(real_pid)).is_some();
        return if found { 0 } else { ESRCH };
    }

    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };

    match send_signal_number_to_pid(Pid(real_pid), sig as u8) {
        Ok(()) => 0,
        Err(SendError::PermissionDenied) => EPERM,
        Err(_) => ESRCH,
    }
}

/// `tgkill(tgid, tid, sig)` -> 0 or errno.
///
/// Sends a signal to a specific thread (tid) in the group (tgid).
/// Phase 3: targets the main thread of the process.
pub fn sys_tgkill(tgid: u64, tid: u64, signum: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::process::signal::delivery::send_signal_to_tcb;
    use crate::process::signal::queue::SigInfo;

    if tgid == 0 || tgid >= 4_194_304 {
        return ESRCH;
    }
    if tid == 0 || tid >= 4_194_304 {
        return ESRCH;
    }

    // Read caller PID to fill SigInfo.
    // SAFETY: kernel GS active in syscall context.
    let sender_pid: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nostack, nomem));
        if ptr == 0 {
            return EFAULT;
        }
        (*(ptr as *const ThreadControlBlock)).pid.0
    };

    let pcb = match PROCESS_REGISTRY.find_by_pid(Pid(tgid as u32)) {
        Some(p) => p,
        None => return ESRCH,
    };

    // Verify POSIX privileges for tgkill
    let target_pid = tgid as u32;
    if sender_pid != target_pid {
        let caller_pcb = match PROCESS_REGISTRY.find_by_pid(Pid(sender_pid)) {
            Some(p) => p,
            None => return EPERM,
        };
        let c_creds = caller_pcb.get_creds();
        let t_creds = pcb.get_creds();
        let is_privileged = c_creds.is_root() || c_creds.euid == 0;
        let uid_match = c_creds.uid == t_creds.uid
            || c_creds.euid == t_creds.uid
            || c_creds.euid == t_creds.euid;
        if !is_privileged && !uid_match {
            return EPERM;
        }
    }

    // Target specific thread `tid` if it exists in the group, otherwise fallback if tid == tgid or 0.
    let mut thread_ptr = pcb.find_thread_by_tid(crate::process::core::pid::Tid(tid as u32));
    if thread_ptr.is_null() && (tid == tgid || tid == 0) {
        thread_ptr = pcb.find_alive_thread();
    }
    if thread_ptr.is_null() {
        return ESRCH;
    }

    if signum == 0 {
        return 0;
    }

    let sig = match validate_signal(signum) {
        Ok(s) => s,
        Err(e) => return e.to_errno(),
    };

    // SAFETY: thread_ptr retained by PCB.
    let thread = unsafe { &*thread_ptr };
    let info = SigInfo::from_kill(sig as u8, sender_pid, 0);
    send_signal_to_tcb(thread, sig as u8, info);
    0
}

/// `sigaltstack(ss, oss)` -> 0 or errno.
///
/// Reads/writes current thread sigaltstack.
/// Layout `struct stack_t` x86_64 (24 bytes):
///   [0]  ss_sp    (u64) — base address of alternate stack
///   [8]  ss_flags (i32) — SS_ONSTACK=1, SS_DISABLE=2
///   [12] _pad     (u32)
///   [16] ss_size  (u64) — size in bytes
pub fn sys_sigaltstack(ss_ptr: u64, oss_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::process::signal::handler::{SigAltStack, SS_DISABLE};

    if ss_ptr != 0 && ss_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if oss_ptr != 0 && oss_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }

    // Read current TCB from gs:[0x20].
    // SAFETY: kernel GS active during a syscall.
    let tcb_ptr = unsafe { current_tcb_ptr() };
    if tcb_ptr.is_null() {
        return ENOSYS;
    }

    let pid = unsafe { Pid((*tcb_ptr).pid.0) };
    let pcb = match PROCESS_REGISTRY.find_by_pid(pid) {
        Some(p) => p,
        None => return EFAULT,
    };

    let target_tid = crate::process::core::pid::Tid(unsafe { (*tcb_ptr).tid as u32 });
    let mut thread_ptr = pcb.find_thread_by_tid(target_tid);
    if thread_ptr.is_null() {
        thread_ptr = pcb.find_alive_thread();
    }
    if thread_ptr.is_null() {
        return EFAULT;
    }

    // SAFETY: thread_ptr retained by PCB; caller = current thread.
    let thread = unsafe { &mut *thread_ptr };

    // Export old sigaltstack if oss_ptr is provided.
    if oss_ptr != 0 {
        let disabled = thread.addresses.sigaltstack_top() == 0;
        let old = SigAltStack {
            ss_sp: if disabled { 0 } else { thread.addresses.sigaltstack_base },
            ss_flags: if disabled { SS_DISABLE } else { 0 },
            _pad: 0,
            ss_size: if disabled { 0 } else { thread.addresses.sigaltstack_size },
        };
        if write_user_typed(oss_ptr, old).is_err() {
            return EFAULT;
        }
    }

    // Install new sigaltstack if ss_ptr is provided.
    if ss_ptr != 0 {
        let ss = match read_user_typed::<SigAltStack>(ss_ptr) {
            Ok(ss) => ss,
            Err(_) => return EFAULT,
        };
        const MINSIGSTKSZ: u64 = 2048;

        if ss.ss_flags & SS_DISABLE != 0 {
            // SS_DISABLE: disable current sigaltstack.
            thread.addresses.sigaltstack_base = 0;
            thread.addresses.sigaltstack_size = 0;
        } else if ss.ss_flags != 0 {
            return EINVAL; // unknown flags
        } else {
            if ss.ss_size < MINSIGSTKSZ {
                return EINVAL;
            }
            thread.addresses.sigaltstack_base = ss.ss_sp;
            thread.addresses.sigaltstack_size = ss.ss_size;
        }
    }

    0
}
