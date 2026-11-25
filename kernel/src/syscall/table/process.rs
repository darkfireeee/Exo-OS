//! # syscall/table/process.rs — Process lifecycle handlers (fork, exec, wait, uid/gid)

use super::common::*;
use crate::syscall::numbers::*;
use crate::syscall::validation::read_user_path;

// ─────────────────────────────────────────────────────────────────────────────
// Process / Thread handlers (delegated to process/)
// ─────────────────────────────────────────────────────────────────────────────

/// `fork()` -> child PID in parent, 0 in child, or errno.
///
/// # PATCH-P0-FORK-STUB
/// This handler is **dead code** for SYS_FORK.
/// dispatch.rs intercepts SYS_FORK at step [5b] via `handle_fork_like_inplace()`
/// BEFORE reaching the slow-path table. `do_fork()` in process/lifecycle/fork.rs
/// is the actual implementation. This stub will never be called in production.
/// It is preserved here as an explicit safety net.
pub fn sys_fork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FORK);
    // STRATA-DISPATCH-01: this point is normally unreachable.
    // If reached, it is a bug in dispatch — return ENOSYS explicitly.
    ENOSYS
}

/// `vfork()` — intercepted by dispatch.rs [5b] via `handle_fork_like_inplace(ForkFlags::VFORK)`.
///
/// # PATCH-P0-FORK-STUB
/// Dead code identical to sys_fork. See comment above.
pub fn sys_vfork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_VFORK);
    // STRATA-DISPATCH-01: this point is normally unreachable.
    ENOSYS
}

/// `clone(flags, stack, ptid, ctid, tls)` — intercepted by dispatch.rs [5b] via
/// `handle_clone_inplace()`.
///
/// Safety net identical to `sys_fork` / `sys_vfork`: if reached outside [5b],
/// return `ENOSYS` rather than diverging from the register-frame clone ABI.
pub fn sys_clone(_flags: u64, _stack: u64, _ptid: u64, _ctid: u64, _tls: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLONE);
    ENOSYS
}

/// `execve(path, argv, envp)` — intercepted by dispatch.rs [5c] via `handle_execve_inplace()`.
///
/// # PATCH-P0-FORK-STUB
/// This handler is **dead code** for SYS_EXECVE.
/// dispatch.rs intercepts SYS_EXECVE at step [5c] BEFORE reaching the table.
/// `do_execve()` in process/lifecycle/exec.rs is the actual implementation.
/// This stub will never be called in production.
pub fn sys_execve(
    path_ptr: u64,
    argv_ptr: u64,
    envp_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXECVE);
    // STRATA-DISPATCH-01: this point is normally unreachable.
    // Path reading is retained to avoid unused_variable warning.
    let _ = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let _ = (argv_ptr, envp_ptr);
    ENOSYS
}

/// `exit(status)` — terminates the calling thread; marks process as zombie only if last thread.
pub fn sys_exit(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXIT);
    let exit_code = (status & 0xFF) as u32;
    // SAFETY: GS:[0x20] is current thread TCB initialized by context_switch.
    // Call is valid from syscall context (kernel GS active after SWAPGS).
    unsafe {
        let tcb_ptr = crate::arch::x86_64::smp::percpu::read_current_tcb();
        if tcb_ptr != 0 {
            let tcb = &*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock);
            let pid = crate::process::core::pid::Pid(tcb.pid.0);
            let tid = crate::process::core::pid::Tid(tcb.tid as u32);
            if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) {
                let thread_ptr = pcb.find_thread_by_tid(tid);
                if !thread_ptr.is_null() {
                    crate::process::lifecycle::exit::do_exit(&mut *thread_ptr, pcb, exit_code);
                } else {
                    use core::sync::atomic::Ordering;
                    let remaining = pcb.dec_threads();
                    if remaining == 0 {
                        let ppid = pcb.ppid();
                        pcb.set_exiting();
                        pcb.exit_code.store(exit_code, Ordering::Release);
                        pcb.flags.fetch_or(
                            crate::process::core::pcb::process_flags::VFORK_DONE,
                            Ordering::Release,
                        );
                        pcb.set_state(crate::process::core::pcb::ProcessState::Zombie);
                        if ppid.0 != 0 {
                            let _ = crate::process::signal::delivery::send_signal_to_pid(
                                ppid,
                                crate::process::signal::default::Signal::SIGCHLD,
                            );
                        }
                        crate::process::lifecycle::wait::wake_waiting_parents(pcb.pid, ppid);
                        crate::process::lifecycle::fork::notify_vfork_completion(pid);
                    }
                }
            }

            // Transition Dead -> pick_next_task will ignore this thread.
            tcb.mark_exiting();
            tcb.set_state(crate::scheduler::core::task::TaskState::Dead);
            let cpu_id = tcb.current_cpu();
            // schedule_block selects next thread and performs context switch.
            crate::scheduler::core::switch::schedule_block(cpu_id, &mut *(tcb_ptr as *mut _));
        }
    }
    // Unreachable after schedule_block with Dead state (satisfies -> i64 type).
    #[allow(clippy::empty_loop)]
    loop {
        unsafe {
            crate::arch::x86_64::halt_once();
        }
    }
}

/// `exit_group(status)` — terminates all threads in process group.
pub fn sys_exit_group(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXIT_GROUP);
    let exit_code = (status & 0xFF) as u32;

    // SAFETY: GS:[0x20] is current TCB during syscall.
    unsafe {
        let tcb_ptr = crate::arch::x86_64::smp::percpu::read_current_tcb();
        if tcb_ptr == 0 {
            return EFAULT;
        }
        let current_tcb = &mut *(tcb_ptr as *mut crate::scheduler::core::task::ThreadControlBlock);
        let pid = crate::process::core::pid::Pid(current_tcb.pid.0);
        let tid = crate::process::core::pid::Tid(current_tcb.tid as u32);
        let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) else {
            return -3;
        };

        pcb.set_exiting();
        pcb.flags.fetch_or(
            crate::process::core::pcb::process_flags::GROUP_EXITING,
            core::sync::atomic::Ordering::Release,
        );
        pcb.exit_code
            .store(exit_code, core::sync::atomic::Ordering::Release);

        let self_thread_ptr = pcb.find_thread_by_tid(tid);

        // Terminate sibling threads safely on their own CPUs via SIGKILL + wakeup/IPI
        // rather than freeing live kernel stacks from underneath running cores.
        pcb.for_each_thread_ptr(|thread_ptr| {
            if thread_ptr == self_thread_ptr {
                return;
            }
            let thread = &mut *thread_ptr;
            thread
                .join_result
                .store(exit_code as u64, core::sync::atomic::Ordering::Release);
            thread
                .join_done
                .store(true, core::sync::atomic::Ordering::Release);
            crate::scheduler::timer::sleep::cancel_sleep_timer_for_tcb(&thread.sched_tcb);
            let sigkill = crate::process::signal::default::Signal::SIGKILL.number();
            crate::process::signal::delivery::send_signal_to_tcb(
                thread,
                sigkill,
                crate::process::signal::queue::SigInfo::kernel(sigkill),
            );
        });
        crate::process::thread::join::wake_joiners();

        if !self_thread_ptr.is_null() {
            crate::process::lifecycle::exit::do_exit(&mut *self_thread_ptr, pcb, exit_code);
        }

        // Fallback when current TCB has no registered ProcessThread slot.
        pcb.flags.fetch_or(
            crate::process::core::pcb::process_flags::VFORK_DONE,
            core::sync::atomic::Ordering::Release,
        );
        if alloc::sync::Arc::strong_count(&pcb.files) == 1 {
            let mut files = pcb.files.lock();
            files.close_all_noalloc();
        }
        crate::process::lifecycle::exit::close_all_pid_vfs(pcb.pid.0);
        crate::drivers::driver_do_exit(pcb.pid.0);
        let _ = crate::security::unregister_service(pcb.pid);
        crate::security::zero_trust::clear_process_restrictions(pcb.pid.0);

        current_tcb.mark_exiting();
        current_tcb.set_state(crate::scheduler::core::task::TaskState::Dead);
        pcb.thread_count
            .store(0, core::sync::atomic::Ordering::Release);
        pcb.set_state(crate::process::core::pcb::ProcessState::Zombie);
        let ppid = pcb.ppid();
        if ppid.0 != 0 {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                ppid,
                crate::process::signal::default::Signal::SIGCHLD,
            );
        }
        crate::process::lifecycle::wait::wake_waiting_parents(pcb.pid, ppid);
        crate::process::lifecycle::fork::notify_vfork_completion(pcb.pid);

        let cpu_id = current_tcb.current_cpu();
        crate::scheduler::core::switch::schedule_block(cpu_id, current_tcb);
    }

    loop {
        unsafe {
            crate::arch::x86_64::halt_once();
        }
    }
}

/// `wait4(pid, wstatus, options, rusage)`.
/// do_waitpid(caller_pid, wait_pid, WaitOptions, &tcb) wired during integration.
pub fn sys_wait4(
    pid: u64,
    wstatus_ptr: u64,
    options: u64,
    rusage_ptr: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_WAIT4);
    crate::syscall::handlers::process::sys_wait4(pid, wstatus_ptr, options, rusage_ptr, 0, 0)
}

/// `waitid(idtype, id, infop, options, rusage)`.
pub fn sys_waitid(
    idtype: u64,
    id: u64,
    infop: u64,
    options: u64,
    rusage_ptr: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_WAITID);
    crate::syscall::handlers::process::sys_waitid(idtype, id, infop, options, rusage_ptr, 0)
}

