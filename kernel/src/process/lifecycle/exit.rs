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
    // DIAG-X1 (bisect hang boot) : détecter une mort silencieuse d'init (PID 1)
    // ou d'ipc_router (PID 2) pendant le boot — sans ce marqueur, un exit
    // prématuré d'init laisse le système vivant mais figé (plus aucun log
    // userspace, seuls les kthreads tournent encore).
    #[cfg(target_arch = "x86_64")]
    if pcb.pid.0 <= 2 {
        crate::arch::x86_64::terminal::debug_write(b"<X1 pid=");
        let digit = b'0' + (pcb.pid.0 as u8).min(9);
        crate::arch::x86_64::terminal::debug_write(&[digit]);
        crate::arch::x86_64::terminal::debug_write(b" code=0x");
        let mut v = exit_status as u64;
        let mut buf = [0u8; 8];
        let mut i = 8usize;
        while i > 0 {
            i -= 1;
            let nib = (v & 0xf) as u8;
            buf[i] = if nib < 10 { b'0' + nib } else { b'a' + nib - 10 };
            v >>= 4;
        }
        crate::arch::x86_64::terminal::debug_write(&buf);
        crate::arch::x86_64::terminal::debug_write(b">\n");
    }
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
        // FIX-APP-08 (Security_Application_Audit §GAP-08) : tracer la terminaison
        // du processus dans ExoLedger. process spawn/exit n'étaient pas audités.
        // ActionTag::Custom { tag = 0x4558_4954 "EXIT", data = pid|status<<32 }.
        crate::security::exoledger::exo_ledger_append(
            crate::security::exoledger::ActionTag::Custom {
                tag: 0x4558_4954, // "EXIT"
                data: (pcb.pid.0 as u64) | ((exit_status as u64) << 32),
            },
        );
        // FIX-P1-VEIL (Security_Application_Audit §GAP-07) : la révocation des
        // capabilities du processus est portée par `pcb.cap_table` (Box détenue
        // par le PCB), libérée au reap — les capabilities par-processus meurent
        // donc avec le PCB sans appel explicite. `exoveil::revoke_domain()` n'est
        // PAS utilisé ici : c'est une primitive de lockdown PKS GLOBAL (décision
        // Kernel B), pas un cleanup per-process — l'invoquer à chaque exit
        // verrouillerait les tables de capabilities de tout le système.
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
