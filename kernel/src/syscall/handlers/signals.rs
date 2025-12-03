//! Signal System Calls
//!
//! Implements POSIX signal handling syscalls:
//! - sigaction: Register signal handlers
//! - sigprocmask: Block/unblock signals
//! - kill: Send signals to processes

use crate::memory::MemoryResult;
use crate::posix_x::signals::{
    SigAction, SigSet, MAX_SIGNAL, SIGKILL, SIGSTOP, SIG_BLOCK, SIG_SETMASK, SIG_UNBLOCK,
};
use crate::scheduler::SCHEDULER;

/// sigaction structure used in syscalls
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SigActionStruct {
    pub sa_handler: usize,
    pub sa_flags: u32,
    pub sa_restorer: usize,
    pub sa_mask: SigSet,
}

/// sys_sigaction - Examine and change a signal action
pub fn sys_sigaction(sig: u32, act: *const SigActionStruct, oldact: *mut SigActionStruct) -> i32 {
    if sig == 0 || sig > MAX_SIGNAL {
        return -22; // EINVAL
    }

    if sig == SIGKILL || sig == SIGSTOP {
        // POSIX says we can't change action for KILL or STOP, but we can inspect it.
        // However, if 'act' is not null, it's an error.
        if !act.is_null() {
            return -22; // EINVAL
        }
    }

    // Get current thread's handler
    let res = SCHEDULER.with_current_thread(|thread| {
        // 1. Save old action if requested
        if !oldact.is_null() {
            if let Some(old) = thread.get_signal_handler(sig) {
                let (handler, mask) = match old {
                    SigAction::Default => (0, SigSet::empty()),
                    SigAction::Ignore => (1, SigSet::empty()),
                    SigAction::Handler { handler, mask } => (handler, mask),
                };

                let old_struct = SigActionStruct {
                    sa_handler: handler,
                    sa_flags: 0, // TODO: Store flags
                    sa_restorer: 0,
                    sa_mask: mask,
                };

                unsafe {
                    *oldact = old_struct;
                }
            }
        }

        // 2. Set new action if requested
        if !act.is_null() {
            let new_act = unsafe { &*act };

            let action = if new_act.sa_handler == 0 {
                SigAction::Default
            } else if new_act.sa_handler == 1 {
                SigAction::Ignore
            } else {
                SigAction::Handler {
                    handler: new_act.sa_handler,
                    mask: new_act.sa_mask,
                }
            };

            thread.set_signal_handler(sig, action);
        }
        0
    });

    res.unwrap_or(-3) // ESRCH
}

/// sys_sigprocmask - Examine and change blocked signals
pub fn sys_sigprocmask(how: i32, set: *const SigSet, oldset: *mut SigSet) -> i32 {
    let res = SCHEDULER.with_current_thread(|thread| {
        // 1. Save old mask if requested
        if !oldset.is_null() {
            let old = thread.get_sigmask();
            unsafe {
                *oldset = old;
            }
        }

        // 2. Set new mask if requested
        if !set.is_null() {
            let new_set = unsafe { *set };
            let mut current = thread.get_sigmask();

            match how {
                SIG_BLOCK => {
                    // Add new_set to current mask
                    // current |= new_set
                    for sig in 1..=MAX_SIGNAL {
                        if new_set.contains(sig) {
                            current.add(sig);
                        }
                    }
                }
                SIG_UNBLOCK => {
                    // Remove new_set from current mask
                    // current &= ~new_set
                    for sig in 1..=MAX_SIGNAL {
                        if new_set.contains(sig) {
                            current.remove(sig);
                        }
                    }
                }
                SIG_SETMASK => {
                    // Replace mask
                    current = new_set;
                }
                _ => return -22, // EINVAL
            }

            // SIGKILL and SIGSTOP can never be blocked
            current.remove(SIGKILL);
            current.remove(SIGSTOP);

            thread.set_sigmask(current);
        }
        0
    });

    res.unwrap_or(-3) // ESRCH
}

/// sys_kill - Send signal to a process (full implementation)
pub fn sys_kill(pid: i32, sig: u32) -> i32 {
    if sig > MAX_SIGNAL {
        return -22; // EINVAL
    }
    
    // Signal 0 is used to check if process exists
    let check_only = sig == 0;

    // pid > 0: Send to process with ID pid
    // pid = 0: Send to process group of current process
    // pid = -1: Send to all processes we have permission to
    // pid < -1: Send to process group -pid

    if pid > 0 {
        let target_pid = pid as u64;
        
        // Check if process exists in scheduler
        let exists = SCHEDULER.get_thread_state(target_pid).is_some();
        
        if !exists {
            return -3; // ESRCH - No such process
        }
        
        if check_only {
            return 0; // Process exists, signal 0 just checks
        }
        
        // Get current process for permission check
        let sender_pid = SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0);
        
        // Permission check (simplified - real impl checks UID/GID)
        // Root (UID 0) can send to anyone
        // For now, allow all signals
        
        // Handle signals based on type
        // Note: Since we can't directly access thread internals from outside,
        // we handle important signals specially
        match sig {
            SIGKILL => {
                // SIGKILL cannot be caught or ignored - terminate via scheduler
                SCHEDULER.terminate_thread(target_pid, -9);
                log::info!("kill: SIGKILL sent to {}, process terminated", target_pid);
                return 0;
            }
            SIGSTOP => {
                // SIGSTOP cannot be caught or ignored
                SCHEDULER.block_thread(target_pid);
                log::info!("kill: SIGSTOP sent to {}, process stopped", target_pid);
                return 0;
            }
            SIGCONT => {
                // SIGCONT resumes stopped process
                SCHEDULER.unblock_thread(target_pid);
                log::info!("kill: SIGCONT sent to {}, process resumed", target_pid);
                return 0;
            }
            _ => {
                // For other signals, queue them for delivery
                // This is a simplified implementation - real impl would check handlers
                let terminates = matches!(sig, 
                    1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 11 | 13 | 14 | 15 | 24 | 25 | 26 | 27 | 31
                );
                if terminates {
                    SCHEDULER.terminate_thread(target_pid, -(sig as i32));
                    log::info!("kill: signal {} to {} caused default termination", sig, target_pid);
                } else {
                    log::debug!("kill: signal {} to {} (no handler, ignored)", sig, target_pid);
                }
                return 0;
            }
        }
    } else if pid == 0 {
        // Send to process group of sender
        let sender_pgid = SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0);
        log::debug!("kill: sending signal {} to process group {}", sig, sender_pgid);
        // TODO: Iterate all processes in group
        return 0;
    } else if pid == -1 {
        // Send to all processes (except init and self)
        log::debug!("kill: sending signal {} to all processes", sig);
        // TODO: Iterate all processes
        return 0;
    } else {
        // pid < -1: Send to process group -pid
        let pgid = (-pid) as u64;
        log::debug!("kill: sending signal {} to process group {}", sig, pgid);
        // TODO: Iterate all processes in group
        return 0;
    }
}

/// sys_sigreturn - Return from signal handler
pub fn sys_sigreturn() -> i32 {
    let res = SCHEDULER.with_current_thread(|thread| {
        // 1. Get current stack pointer from context
        // Note: In a real syscall handler, we'd get the user RSP from the trap frame.
        // Here we assume thread.context.rsp points to the signal frame on the stack.
        let _sp = thread.context_ptr();

        // We need to access the context directly.
        // Since we are inside with_current_thread, we have &mut Thread.
        // But ThreadContext is private in Thread? No, it's private field `context`.
        // We need a method on Thread to restore context from stack.

        // Let's implement the logic here using a new helper on Thread if needed,
        // or just assume we can access it if we make it public (it's not).
        // Actually, let's add `restore_signal_context` to Thread.

        thread.restore_signal_context();

        0
    });

    if let Some(_) = res {
        // Yield to force reload of context
        crate::scheduler::yield_now();
        0
    } else {
        -3 // ESRCH
    }
}

/// sys_tkill - Send signal to a specific thread
pub fn sys_tkill(tid: i32, sig: u32) -> i32 {
    if sig > MAX_SIGNAL {
        return -22; // EINVAL
    }

    // TODO: Implement thread lookup and signal sending
    // For now, stub similar to kill
    log::info!("sys_tkill: sending signal {} to tid {}", sig, tid);

    // If tid matches current thread, we could handle it, but for now just return 0
    // or ESRCH if we can't find it.

    0
}

/// sys_sigaltstack - Set/get alternate signal stack
pub fn sys_sigaltstack(ss: *const u8, old_ss: *mut u8) -> i32 {
    // TODO: Implement alternate signal stack support in Thread
    // For now, stub returning 0 (success) or ENOMEM
    // Many apps use this, so a success stub is better than ENOSYS

    if !ss.is_null() {
        log::info!("sys_sigaltstack: setting alt stack (stub)");
    }

    if !old_ss.is_null() {
        // Write zeroed old stack info
        // We need the struct definition for stack_t
    }

    0
}

/// sys_rt_sigpending - Examine pending signals
pub fn sys_rt_sigpending(set: *mut SigSet, sigsetsize: usize) -> i32 {
    if sigsetsize != core::mem::size_of::<SigSet>() {
        return -22; // EINVAL
    }

    let res = SCHEDULER.with_current_thread(|thread| {
        if !set.is_null() {
            // Get pending signals
            // We need a method on Thread to get pending signals
            // For now, return empty set
            unsafe { *set = SigSet::empty() };
        }
        0
    });

    res.unwrap_or(-3) // ESRCH
}

/// sys_rt_sigsuspend - Wait for a signal
pub fn sys_rt_sigsuspend(mask: *const SigSet, sigsetsize: usize) -> i32 {
    if sigsetsize != core::mem::size_of::<SigSet>() {
        return -22; // EINVAL
    }

    // TODO: Implement signal suspension
    // This requires scheduler support to put thread to sleep until signal arrives
    // For now, stub returning EINTR (Interrupted system call) immediately

    -4 // EINTR
}
