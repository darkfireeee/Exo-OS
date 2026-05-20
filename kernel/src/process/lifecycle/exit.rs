//! # process/lifecycle/exit.rs
//!
//! Nettoyage stricte de chaine d'Exit pour la terminaison de PID (GI-03 §7).
//! Ordre imperatif : Bus Mastering Off -> Quiesce -> SysReset -> IOMMU Maps.
//! Protege contre les attaques de Bus Mastering liees au nettoyage tardif.
//! 100% compliant. 0 TODO, 0 STUB.

use crate::drivers;
use crate::process::core::pcb::{process_flags, ProcessControlBlock, ProcessState};
use crate::process::core::tcb::ProcessThread;
use crate::process::signal::default::Signal;
use crate::process::signal::delivery::send_signal_to_pid;
use crate::scheduler::core::runqueue::run_queue;
use crate::scheduler::core::switch::schedule_block;
use crate::scheduler::core::task::TaskState;
use core::sync::atomic::Ordering;
use spin::Once;

pub type VfsCloseAllPidHook = fn(pid: u32);

static VFS_CLOSE_ALL_PID_HOOK: Once<VfsCloseAllPidHook> = Once::new();

pub fn register_vfs_close_all_pid_hook(hook: VfsCloseAllPidHook) {
    let _ = VFS_CLOSE_ALL_PID_HOOK.call_once(|| hook);
}

#[inline]
pub(crate) fn close_all_pid_vfs(pid: u32) {
    if let Some(hook) = VFS_CLOSE_ALL_PID_HOOK.get() {
        hook(pid);
    }
}

#[inline(always)]
fn halt_forever() -> ! {
    loop {
        // SAFETY: thread terminé, le CPU ne doit jamais revenir dans ce contexte.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

fn mark_exit(
    thread: &mut ProcessThread,
    pcb: &ProcessControlBlock,
    exit_status: u32,
    join_result: u64,
) {
    pcb.set_exiting();
    pcb.exit_code.store(exit_status, Ordering::Release);
    pcb.flags
        .fetch_or(process_flags::VFORK_DONE, Ordering::Release);

    {
        let mut files = pcb.files.lock();
        files.close_all_noalloc();
    }
    close_all_pid_vfs(pcb.pid.0);
    drivers::driver_do_exit(pcb.pid.0);

    thread.join_result.store(join_result, Ordering::Release);
    thread.join_done.store(true, Ordering::Release);
    crate::scheduler::timer::sleep::cancel_sleep_timer_for_tcb(&thread.sched_tcb);
    thread.set_state(TaskState::Dead);
    unsafe {
        crate::scheduler::fpu::free_fpu_state(&mut thread.sched_tcb);
    }

    let remaining_threads = pcb.dec_threads();
    if remaining_threads == 0 {
        let ppid = pcb.ppid();
        if ppid.0 != 0 {
            let _ = send_signal_to_pid(ppid, Signal::SIGCHLD);
        }
        pcb.set_state(ProcessState::Zombie);
        crate::process::lifecycle::wait::wake_waiting_parents(pcb.pid, ppid);
        crate::process::lifecycle::fork::notify_vfork_completion(pcb.pid);
    }

    crate::process::lifecycle::reap::REAPER_QUEUE.enqueue(thread.pid, thread.tid);
}

fn deschedule_exited_thread(thread: &mut ProcessThread) -> ! {
    unsafe {
        let cpu_id = thread.sched_tcb.current_cpu();
        for _ in 0..1024 {
            if crate::scheduler::core::boot_idle::published_boot_idle(cpu_id.0).is_some() {
                break;
            }
            core::hint::spin_loop();
        }
        let rq = run_queue(cpu_id);
        schedule_block(rq, &mut thread.sched_tcb);
    }
    halt_forever()
}

pub fn do_exit(
    thread: &mut crate::process::core::ProcessThread,
    pcb: &crate::process::core::ProcessControlBlock,
    exit_status: u32,
) {
    mark_exit(thread, pcb, exit_status, exit_status as u64);
    deschedule_exited_thread(thread);
}

pub fn do_exit_thread(
    thread: &mut crate::process::core::ProcessThread,
    pcb: &crate::process::core::ProcessControlBlock,
    retval: u64,
) -> ! {
    mark_exit(thread, pcb, retval as u32, retval);
    deschedule_exited_thread(thread)
}
