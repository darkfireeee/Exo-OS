//! # syscall/handlers/process.rs — Process thin wrappers (fork, exec, exit, wait)
//!
//! RULE SYS-03: THIN WRAPPERS ONLY — zero business logic.
//! RULE FORK-02: CoW kernel-side with page_table_lock + TLB shootdown.
//! RULE EXEC-01: copy_from_user for path, argv, envp BEFORE any processing.
//! RULE BUG-04: do_exec() must initialize %fs (TLS) before jump_to_entry.
//! RULE BUG-09: block_all_except_kill() during exec to avoid inter-exec signal.

use crate::syscall::errno::{EAGAIN, ECHILD, EFAULT, EINTR, EINVAL, ENOMEM, ENOSYS};
use crate::syscall::validation::{
    copy_to_user, read_user_path, read_user_typed, write_user_typed, USER_ADDR_MAX,
};

// ─────────────────────────────────────────────────────────────────────────────
// fork() — Ring3 -> kernel CoW
// ─────────────────────────────────────────────────────────────────────────────

/// `fork()` -> child PID in parent, 0 in child, or errno.
///
/// FORK-02: page_table_lock held during mark_all_pages_cow + TLB shootdown.
/// FORK-03: TLB shootdown IPI to ALL active CPUs of the process.
/// FORK-05: child.SignalTcb.pending = 0.
/// FORK-09: FORBIDDEN to flush userspace write buffers here — handled by exo-libc (fflush).
pub fn sys_fork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // Delegates -> process::lifecycle::fork::do_fork()
    // do_fork(): clone PCB, CoW page tables, caps, TLB shootdown, child TCB.
    // NOTE: exo-rt setup_child_stack + TCB + atfork handlers are Ring3.
    // IMPORTANT NOTE (2026-03): the active syscall path for fork goes through
    // `syscall::dispatch::handle_fork_inplace()` (special case SYS_FORK), as
    // it requires direct access to `frame.rcx`/`frame.rsp`. This legacy wrapper
    // is preserved for table compatibility, but is not the primary path.
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// execve() — image replacement
// ─────────────────────────────────────────────────────────────────────────────

/// `execve(path, argv, envp)` -> does not return on success, errno otherwise.
///
/// EXEC-01: copy_from_user() for path, argv, envp BEFORE any use.
/// EXEC-02: check ObjectKind::Code — Blob/Secret not executable.
/// EXEC-04: FD_CLOEXEC automatically revoked — POSIX mandatory.
/// EXEC-07: initial stack aligned to 16 bytes (rsp % 16 == 0) before entry call.
/// EXEC-08: AT_PHDR, AT_PHNUM, AT_ENTRY, AT_RANDOM, AT_SIGNAL_TCB, AT_SYSINFO_EHDR.
/// BUG-04: arch::set_fs_base(tls_initial_addr) before jump_to_entry — PROC-10.
/// BUG-09: block_all_except_kill() BEFORE ELF loading — PROC-03.
pub fn sys_execve(
    path_ptr: u64,
    argv_ptr: u64,
    envp_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    // SYS-01: copy_from_user for path (MANDATORY before any access)
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    // Validate argv_ptr and envp_ptr (userspace pointers)
    if argv_ptr != 0 && argv_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if envp_ptr != 0 && envp_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    // Delegates -> process::lifecycle::exec::do_execve()
    // do_execve() follows the mandatory sequence of document §4.3:
    //   1. copy_from_user path/argv/envp
    //   2. path_resolve + verify ObjectKind::Code
    //   3. block_all_except_kill() <- BUG-09 fix
    //   4. validate_and_load ELF (magic 0x7F ELF first — EXEC-11)
    //   5. revoke_cloexec() <- EXEC-04
    //   6. apply_exec_policy (caps)
    //   7. reset_for_exec() (TCB signals -> SIG_DFL)
    //   8. setup_initial_stack + push_auxv
    //   9. set_fs_base(tls_initial_addr) <- BUG-04 fix, PROC-10
    //  10. jump_to_entry (never returns)
    // IMPORTANT NOTE (2026-03): the active syscall path for execve goes through
    // `syscall::dispatch::handle_execve_inplace()` (special case SYS_EXECVE),
    // to directly update the SyscallFrame (userspace RIP/RSP).
    // This legacy wrapper intentionally remains an unwired fallback.
    let _ = (path, argv_ptr, envp_ptr);
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// exit() / exit_group()
// ─────────────────────────────────────────────────────────────────────────────

/// `exit(status)` — terminates the calling thread; marks process as zombie only if last thread.
pub fn sys_exit(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    crate::syscall::table::process::sys_exit(status, 0, 0, 0, 0, 0)
}

/// `exit_group(status)` — terminates all threads in the group.
pub fn sys_exit_group(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    crate::syscall::table::process::sys_exit_group(status, 0, 0, 0, 0, 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// wait4() / waitid()
// ─────────────────────────────────────────────────────────────────────────────

/// `wait4(pid, status, options, rusage)` -> collected PID or errno.
pub fn sys_wait4(pid: u64, status_ptr: u64, options: u64, _rusage: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::process::core::pid::Pid;
    use crate::process::lifecycle::wait::{do_waitpid, WaitError, WaitOptions};
    use crate::scheduler::core::task::ThreadControlBlock;

    if status_ptr != 0 && status_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }

    // Read TCB of current thread from gs:[0x20].
    // SAFETY: kernel GS active in syscall context.
    let tcb_ptr: u64 = unsafe {
        let p: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) p, options(nostack, nomem));
        p
    };
    if tcb_ptr == 0 {
        return EFAULT;
    }

    // SAFETY: tcb_ptr from gs:[0x20] is always valid during a syscall.
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
    let caller_pid = Pid(tcb.pid.0);
    let wait_pid = pid as i32;
    let opts = WaitOptions(options as u32);

    match do_waitpid(caller_pid, wait_pid, opts, tcb) {
        Ok(result) => {
            // Write wstatus into userspace if status_ptr is non-null.
            if status_ptr != 0 {
                if write_user_typed(status_ptr, result.wstatus).is_err() {
                    return EFAULT;
                }
            }
            result.pid.0 as i64
        }
        // WNOHANG set, children exist but none is zombie: return 0 (POSIX).
        Err(WaitError::WouldBlock) => 0,
        Err(WaitError::NoChild) => ECHILD,
        Err(WaitError::Interrupted) => EINTR,
        Err(WaitError::InvalidPid) => EINVAL,
    }
}

/// `waitid(idtype, id, infop, options, rusage)`.
pub fn sys_waitid(idtype: u64, id: u64, infop: u64, options: u64, _rusage: u64, _a6: u64) -> i64 {
    use crate::process::core::pid::Pid;
    use crate::process::lifecycle::wait::{do_waitpid, WaitError, WaitOptions, WaitReason};
    use crate::scheduler::core::task::ThreadControlBlock;

    if infop != 0 && infop >= USER_ADDR_MAX {
        return EFAULT;
    }

    // Read TCB of current thread from gs:[0x20].
    // SAFETY: kernel GS active in syscall context.
    let tcb_ptr: u64 = unsafe {
        let p: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) p, options(nostack, nomem));
        p
    };
    if tcb_ptr == 0 {
        return EFAULT;
    }

    // SAFETY: tcb_ptr from gs:[0x20] is always valid during a syscall.
    let tcb = unsafe { &*(tcb_ptr as *const ThreadControlBlock) };
    let caller_pid = Pid(tcb.pid.0);

    // idtype: P_ALL=0, P_PID=1, P_PGID=2
    let wait_pid: i32 = match idtype {
        0 => -1,           // P_ALL: any child
        1 => id as i32,    // P_PID: specific child
        2 => -(id as i32), // P_PGID: process group (approx.)
        _ => return EINVAL,
    };

    // Conversion of waitid flags -> WaitOptions.
    // WNOHANG=1, WSTOPPED=2, WEXITED=4, WCONTINUED=8
    let mut wo = WaitOptions(0);
    if options & 0x1 != 0 {
        wo.0 |= WaitOptions::WNOHANG;
    } // WNOHANG
    if options & 0x2 != 0 {
        wo.0 |= WaitOptions::WUNTRACED;
    } // WSTOPPED
    if options & 0x8 != 0 {
        wo.0 |= WaitOptions::WCONTINUED;
    } // WCONTINUED
      // WEXITED=4 is equivalent to normal mode (no additional flag).

    match do_waitpid(caller_pid, wait_pid, wo, tcb) {
        Ok(result) => {
            if infop != 0 {
                // Fill siginfo_t (x86_64 musl/Linux layout):
                // [0]  si_signo (int32) = SIGCHLD = 17
                // [4]  si_errno (int32) = 0
                // [8]  si_code  (int32) = CLD_EXITED=1 or CLD_KILLED=2
                // [12] si_pid   (int32) = child PID
                // [16] si_uid   (uint32) = 0
                // [20] si_status(int32) = raw status code (unshifted)
                // [24..128] = zeroes
                const SIGCHLD: i32 = 17;
                const CLD_EXITED: i32 = 1;
                const CLD_KILLED: i32 = 2;

                let (si_code, si_status) = match result.reason {
                    WaitReason::Exited => (CLD_EXITED, (result.wstatus >> 8) as i32),
                    _ => (CLD_KILLED, (result.wstatus & 0x7F) as i32),
                };

                let mut siginfo = [0u8; 128];
                siginfo[0..4].copy_from_slice(&SIGCHLD.to_ne_bytes());
                siginfo[4..8].copy_from_slice(&0i32.to_ne_bytes());
                siginfo[8..12].copy_from_slice(&si_code.to_ne_bytes());
                siginfo[12..16].copy_from_slice(&(result.pid.0 as i32).to_ne_bytes());
                siginfo[16..20].copy_from_slice(&0u32.to_ne_bytes());
                siginfo[20..24].copy_from_slice(&si_status.to_ne_bytes());
                if copy_to_user(infop as *mut u8, siginfo.as_ptr(), siginfo.len()).is_err() {
                    return EFAULT;
                }
            }
            0 // waitid returns 0 on success (unlike wait4)
        }
        // WNOHANG, children present but none is zombie: success with si_pid=0 (POSIX waitid compliant).
        Err(WaitError::WouldBlock) => {
            if infop != 0 {
                let siginfo = [0u8; 128];
                if copy_to_user(infop as *mut u8, siginfo.as_ptr(), siginfo.len()).is_err() {
                    return EFAULT;
                }
            }
            0
        }
        Err(WaitError::NoChild) => ECHILD,
        Err(WaitError::Interrupted) => EINTR,
        Err(WaitError::InvalidPid) => EINVAL,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// clone() — thread creation
// ─────────────────────────────────────────────────────────────────────────────

/// `clone(flags, stack, ptid, ctid, tls)` -> TID or errno.
pub fn sys_clone(flags: u64, stack: u64, ptid: u64, ctid: u64, tls: u64, entry: u64) -> i64 {
    const CLONE_SETTLS: u64 = 0x0008_0000;

    if ptid != 0 && ptid >= USER_ADDR_MAX {
        return EFAULT;
    }
    if ctid != 0 && ctid >= USER_ADDR_MAX {
        return EFAULT;
    }
    let start_func = if entry != 0 {
        entry
    } else if flags & CLONE_SETTLS == 0 && tls != 0 {
        tls
    } else {
        ctid
    };
    if start_func == 0 {
        return EINVAL;
    }

    let arg = if entry != 0 && stack != 0 {
        match read_user_typed::<u64>(stack) {
            Ok(value) => value,
            Err(e) => return e.to_errno(),
        }
    } else {
        0
    };
    let stack_addr = stack;
    let stack_size = if stack != 0 { 0u64 } else { 8 * 1024 * 1024u64 };
    let detached = (flags & 0x0040_0000) != 0;
    // Read current PID from per-CPU TCB
    let current_pid_val: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 {
            return EFAULT;
        }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock))
            .pid
            .0
    };
    let pcb_ref = match crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(current_pid_val))
    {
        Some(p) => p,
        None => return -3i64, // ESRCH
    };
    let attr = crate::process::thread::creation::ThreadAttr {
        stack_size,
        stack_addr,
        policy: crate::scheduler::core::task::SchedPolicy::Normal,
        priority: crate::scheduler::core::task::Priority::NORMAL_DEFAULT,
        detached,
        cpu_affinity: -1,
        sigaltstack_size: 8192,
    };
    let params = crate::process::thread::creation::ThreadCreateParams {
        pcb: pcb_ref as *const crate::process::core::pcb::ProcessControlBlock,
        attr,
        start_func,
        arg,
        initial_rsp: stack,
        tls_base: if flags & CLONE_SETTLS != 0 { tls } else { 0 },
        target_cpu: unsafe {
            let p = crate::scheduler::core::switch::current_thread_raw();
            if p.is_null() {
                0
            } else {
                (*p).current_cpu().0
            }
        },
        pthread_out: if flags & 0x0010_0000 != 0 { ptid } else { 0 },
        child_tid_ptr: if flags & 0x0100_0000 != 0 { ctid } else { 0 },
        clear_child_tid: if flags & 0x0020_0000 != 0 { ctid } else { 0 },
    };
    match crate::process::thread::creation::create_thread(&params) {
        Ok(handle) => handle.tid.0 as i64,
        Err(crate::process::thread::creation::ThreadCreateError::OutOfMemory) => ENOMEM,
        Err(crate::process::thread::creation::ThreadCreateError::TidExhausted) => EAGAIN,
        Err(_) => EINVAL,
    }
}
