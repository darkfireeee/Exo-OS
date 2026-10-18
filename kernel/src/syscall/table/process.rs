//! # syscall/table/process.rs — Handlers cycle de vie des processus (fork, exec, wait, uid/gid)

use super::common::*;
use crate::syscall::numbers::*;
use crate::syscall::validation::{read_user_path, read_user_typed};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Process / Thread (délégués vers process/)
// ─────────────────────────────────────────────────────────────────────────────

/// `fork()` → PID fils dans le parent, 0 dans le fils, ou errno.
///
/// # PATCH-P0-FORK-STUB
/// Ce handler est du **code mort** pour SYS_FORK.
/// Le dispatch.rs intercepte SYS_FORK à l'étape [5b] via `handle_fork_like_inplace()`
/// AVANT d'atteindre la slow-path table. `do_fork()` dans process/lifecycle/fork.rs
/// est l'implémentation réelle. Ce stub ne sera jamais appelé en production.
/// Il est conservé ici comme filet de sécurité explicite.
pub fn sys_fork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FORK);
    // STRATA-DISPATCH-01: ce point est normalement inatteignable.
    // Si atteint, c'est un bug dans le dispatch — retourner ENOSYS explicitement.
    ENOSYS
}

/// `vfork()` — intercepté par dispatch.rs [5b] via `handle_fork_like_inplace(ForkFlags::VFORK)`.
///
/// # PATCH-P0-FORK-STUB
/// Code mort identique à sys_fork. Voir commentaire ci-dessus.
pub fn sys_vfork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_VFORK);
    // STRATA-DISPATCH-01: ce point est normalement inatteignable.
    ENOSYS
}

/// `clone(flags, stack, ptid, ctid, tls)` — crée un nouveau thread via create_thread.
///
/// Convention Exo-OS :
/// - `stack`     : RSP initial du thread fils et slot `arg` du wrapper musl.
/// - `tls`       : base FS quand `CLONE_SETTLS` est présent.
/// - `entry`     : 6e registre syscall; le wrapper x86_64 musl y porte `func`.
/// - `ctid`/`ptid`: pointeurs TID POSIX conservés pour le protocole clone.
/// - CLONE_DETACHED (0x0040_0000) : thread détaché.
pub fn sys_clone(flags: u64, stack: u64, ptid: u64, ctid: u64, tls: u64, entry: u64) -> i64 {
    stat_inc(SYS_CLONE);

    const CLONE_VM: u64 = 0x0000_0100;
    const CLONE_FS: u64 = 0x0000_0200;
    const CLONE_FILES: u64 = 0x0000_0400;
    const CLONE_SIGHAND: u64 = 0x0000_0800;
    const CLONE_THREAD: u64 = 0x0001_0000;
    const CLONE_SYSVSEM: u64 = 0x0004_0000;
    const CLONE_SETTLS: u64 = 0x0008_0000;
    const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
    const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
    const CLONE_DETACHED: u64 = 0x0040_0000;
    const CLONE_CHILD_SETTID: u64 = 0x0100_0000;
    const SUPPORTED_THREAD_FLAGS: u64 = CLONE_VM
        | CLONE_FS
        | CLONE_FILES
        | CLONE_SIGHAND
        | CLONE_THREAD
        | CLONE_SYSVSEM
        | CLONE_SETTLS
        | CLONE_PARENT_SETTID
        | CLONE_CHILD_CLEARTID
        | CLONE_DETACHED
        | CLONE_CHILD_SETTID;

    let thread_core = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD;
    if flags & CLONE_THREAD == 0 {
        return ENOSYS;
    }
    if flags & thread_core != thread_core {
        return EINVAL;
    }
    if flags & !SUPPORTED_THREAD_FLAGS != 0 {
        return EINVAL;
    }

    // Récupérer le PID du thread courant via GS:[0x20].
    // SAFETY: GS:[0x20] est initialisé par context_switch avant toute entrée syscall.
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

    // Trouver le PCB du processus courant dans le registry global.
    let pcb_ref = match crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(current_pid_val))
    {
        Some(p) => p,
        None => return -3i64, // ESRCH
    };

    // Le wrapper musl x86_64 garde `func` dans r9 pendant SYS_clone. Tolérer
    // l'ancien chemin Exo-OS uniquement quand `tls` n'est pas une base TLS.
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

    // Stack : l'appelant fournit RSP ou on conserve le fallback historique.
    let stack_addr = stack;
    let stack_size = if stack != 0 { 0 } else { 8 * 1024 * 1024 };
    let detached = (flags & CLONE_DETACHED) != 0;

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
        target_cpu: 0,
        pthread_out: ptid,
    };
    match crate::process::thread::creation::create_thread(&params) {
        Ok(handle) => handle.tid.0 as i64,
        Err(crate::process::thread::creation::ThreadCreateError::OutOfMemory) => ENOMEM,
        Err(crate::process::thread::creation::ThreadCreateError::TidExhausted)
        | Err(crate::process::thread::creation::ThreadCreateError::TooManyThreads) => EAGAIN,
        Err(_) => EINVAL,
    }
}

/// `execve(path, argv, envp)` — intercepté par dispatch.rs [5c] via `handle_execve_inplace()`.
///
/// # PATCH-P0-FORK-STUB
/// Ce handler est du **code mort** pour SYS_EXECVE.
/// Le dispatch.rs intercepte SYS_EXECVE à l'étape [5c] AVANT d'atteindre la table.
/// `do_execve()` dans process/lifecycle/exec.rs est l'implémentation réelle.
/// Ce stub ne sera jamais appelé en production.
pub fn sys_execve(
    path_ptr: u64,
    argv_ptr: u64,
    envp_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXECVE);
    // STRATA-DISPATCH-01: ce point est normalement inatteignable.
    // La lecture du path est conservée pour éviter un avertissement unused_variable.
    let _ = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    let _ = (argv_ptr, envp_ptr);
    ENOSYS
}

/// `exit(status)` — marque le processus zombie, réveille le parent, puis cède le CPU.
pub fn sys_exit(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXIT);
    let exit_code = (status & 0xFF) as u32;
    // SAFETY: GS:[0x20] est le TCB du thread courant initialisé par context_switch.
    // L'appel est valide depuis le contexte syscall (kernel GS actif après SWAPGS).
    unsafe {
        let tcb_ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nomem, nostack));
        if tcb_ptr != 0 {
            let tcb = &*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock);

            // Stocker le code de sortie dans le PCB pour waitpid() (BUG-1 FIX).
            // REGISTRY.find_by_pid() est lockless — sûr depuis un contexte syscall.
            let pid = crate::process::core::pid::Pid(tcb.pid.0);
            if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) {
                use core::sync::atomic::Ordering;
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

            // Transition Dead → pick_next_task ignorera ce thread.
            tcb.set_state(crate::scheduler::core::task::TaskState::Dead);
            let cpu_id = tcb.current_cpu();
            let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
            // schedule_block sélectionne le prochain thread et effectue le context switch.
            crate::scheduler::core::switch::schedule_block(rq, &mut *(tcb_ptr as *mut _));
        }
    }
    // Unreachable après schedule_block avec état Dead (satisfait le type -> i64).
    #[allow(clippy::empty_loop)]
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// `exit_group(status)` — termine tous les threads du groupe de processus.
///
/// Délègue vers sys_exit() pour l'instant.
/// L'itération sur tous les threads frères requiert process/ pleinement intégré.
pub fn sys_exit_group(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXIT_GROUP);
    let exit_code = (status & 0xFF) as u32;

    // SAFETY: GS:[0x20] est le TCB courant pendant le syscall.
    unsafe {
        let tcb_ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nomem, nostack));
        if tcb_ptr == 0 {
            return EFAULT;
        }
        let current_tcb = &mut *(tcb_ptr as *mut crate::scheduler::core::task::ThreadControlBlock);
        let pid = crate::process::core::pid::Pid(current_tcb.pid.0);
        let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) else {
            return -3;
        };

        pcb.set_exiting();
        pcb.exit_code
            .store(exit_code, core::sync::atomic::Ordering::Release);
        pcb.flags.fetch_or(
            crate::process::core::pcb::process_flags::VFORK_DONE,
            core::sync::atomic::Ordering::Release,
        );

        {
            let mut files = pcb.files.lock();
            files.close_all_noalloc();
        }
        crate::process::lifecycle::exit::close_all_pid_vfs(pcb.pid.0);
        crate::drivers::driver_do_exit(pcb.pid.0);

        pcb.for_each_thread_ptr(|thread_ptr| {
            let thread = &mut *thread_ptr;
            thread
                .join_result
                .store(exit_code as u64, core::sync::atomic::Ordering::Release);
            thread
                .join_done
                .store(true, core::sync::atomic::Ordering::Release);
            thread.sched_tcb.mark_exiting();
            thread.set_state(crate::scheduler::core::task::TaskState::Dead);
            crate::process::lifecycle::reap::REAPER_QUEUE.enqueue(thread.pid, thread.tid);
        });
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
        let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
        crate::scheduler::core::switch::schedule_block(rq, current_tcb);
    }

    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// `wait4(pid, wstatus, options, rusage)`.
/// do_waitpid(caller_pid, wait_pid, WaitOptions, &tcb) câblé lors de l'intégration.
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

