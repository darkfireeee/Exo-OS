//! # syscall/handlers/process.rs — Thin wrappers process (fork, exec, exit, wait)
//!
//! RÈGLE SYS-03 : THIN WRAPPERS UNIQUEMENT — zéro logique métier.
//! RÈGLE FORK-02 : CoW kernel-side avec page_table_lock + TLB shootdown.
//! RÈGLE EXEC-01 : copy_from_user pour path, argv, envp AVANT tout traitement.
//! RÈGLE BUG-04  : do_exec() doit initialiser %fs (TLS) avant jump_to_entry.
//! RÈGLE BUG-09  : block_all_except_kill() durant exec pour éviter signal inter-exec.

#![allow(dead_code)]

use crate::syscall::validation::{read_user_path, validate_pid, validate_signal, USER_ADDR_MAX};
use crate::syscall::errno::{EINVAL, EFAULT, ENOMEM, ENOSYS, EAGAIN};
use crate::syscall::numbers::*;

// ─────────────────────────────────────────────────────────────────────────────
// fork() — Ring3 → kernel CoW
// ─────────────────────────────────────────────────────────────────────────────

/// `fork()` → PID fils dans le parent, 0 dans le fils, ou errno.
///
/// FORK-02 : page_table_lock tenu pendant mark_all_pages_cow + TLB shootdown.
/// FORK-03 : TLB shootdown IPI à TOUS les CPU actifs du processus.
/// FORK-05 : child.SignalTcb.pending = 0.
/// FORK-09 : INTERDIT de flush write buffers userspace ici — c'est exo-libc (fflush).
pub fn sys_fork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // Délègue → process::lifecycle::fork::do_fork()
    // do_fork() : clone PCB, CoW page tables, caps, TLB shootdown, TCB child.
    // NOTE: exo-rt setup_child_stack + TCB + atfork handlers sont Ring3.
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// execve() — remplacement d'image
// ─────────────────────────────────────────────────────────────────────────────

/// `execve(path, argv, envp)` → ne retourne pas en cas de succès, errno sinon.
///
/// EXEC-01 : copy_from_user() pour path, argv, envp AVANT toute utilisation.
/// EXEC-02 : vérifier ObjectKind::Code — Blob/Secret non exécutables.
/// EXEC-04 : FD_CLOEXEC révoqué automatiquement — POSIX obligatoire.
/// EXEC-07 : stack initiale alignée 16 bytes (rsp % 16 == 0) avant le call entry.
/// EXEC-08 : AT_PHDR, AT_PHNUM, AT_ENTRY, AT_RANDOM, AT_SIGNAL_TCB, AT_SYSINFO_EHDR.
/// BUG-04  : arch::set_fs_base(tls_initial_addr) avant jump_to_entry — PROC-10.
/// BUG-09  : block_all_except_kill() AVANT chargement ELF — PROC-03.
pub fn sys_execve(path_ptr: u64, argv_ptr: u64, envp_ptr: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // SYS-01 : copy_from_user pour le chemin (OBLIGATOIRE avant tout accès)
    let path = match read_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e.to_errno(),
    };
    // Valider argv_ptr et envp_ptr (pointeurs userspace)
    if argv_ptr != 0 && argv_ptr >= USER_ADDR_MAX { return EFAULT; }
    if envp_ptr != 0 && envp_ptr >= USER_ADDR_MAX { return EFAULT; }
    // Délègue → process::lifecycle::exec::do_execve()
    // do_execve() suit la séquence obligatoire du document §4.3 :
    //   1. copy_from_user path/argv/envp
    //   2. path_resolve + verify ObjectKind::Code
    //   3. block_all_except_kill() ← BUG-09 fix
    //   4. validate_and_load ELF (magic 0x7F ELF first — EXEC-11)
    //   5. revoke_cloexec() ← EXEC-04
    //   6. apply_exec_policy (caps)
    //   7. reset_for_exec() (TCB signals → SIG_DFL)
    //   8. setup_initial_stack + push_auxv
    //   9. set_fs_base(tls_initial_addr) ← BUG-04 fix, PROC-10
    //  10. jump_to_entry (jamais de retour)
    let _ = (path, argv_ptr, envp_ptr);
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// exit() / exit_group()
// ─────────────────────────────────────────────────────────────────────────────

/// `exit(status)` — marque le thread Dead et cède le CPU.
pub fn sys_exit(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let exit_code = (status & 0xFF) as u32;
    // Délègue → process::lifecycle::exit::do_exit()
    // do_exit() stocke exit_code dans PCB, met état → Zombie, appelle schedule_block().
    // SAFETY: appelé uniquement depuis le contexte syscall — GS kernel actif.
    unsafe {
        let tcb_ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nomem, nostack));
        if tcb_ptr != 0 {
            let tcb = &*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock);
            let pid = crate::process::core::pid::Pid(tcb.pid.0);
            if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) {
                pcb.exit_code.store(exit_code, core::sync::atomic::Ordering::Release);
                pcb.set_state(crate::process::core::pcb::ProcessState::Zombie);
            }
            tcb.set_state(crate::scheduler::core::task::TaskState::Dead);
            let cpu_id = tcb.current_cpu();
            let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
            crate::scheduler::core::switch::schedule_block(rq, &mut *(tcb_ptr as *mut _));
        }
    }
    // Unreachable après schedule_block avec état Dead.
    loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
}

/// `exit_group(status)` — termine tous les threads du groupe.
pub fn sys_exit_group(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // Délègue vers sys_exit pour l'instant.
    // Itération sur les threads frères require process/ pleinement intégré.
    sys_exit(status, 0, 0, 0, 0, 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// wait4() / waitid()
// ─────────────────────────────────────────────────────────────────────────────

/// `wait4(pid, status, options, rusage)` → PID collecté ou errno.
pub fn sys_wait4(pid: u64, status_ptr: u64, options: u64, _rusage: u64, _a5: u64, _a6: u64) -> i64 {
    let _ = (pid, options);
    if status_ptr != 0 && status_ptr >= USER_ADDR_MAX { return EFAULT; }
    // Délègue → process::lifecycle::wait::do_wait4()
    ENOSYS
}

/// `waitid(idtype, id, infop, options, rusage)`.
pub fn sys_waitid(idtype: u64, id: u64, infop: u64, options: u64, _rusage: u64, _a6: u64) -> i64 {
    if infop != 0 && infop >= USER_ADDR_MAX { return EFAULT; }
    let _ = (idtype, id, options);
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// clone() — création de thread
// ─────────────────────────────────────────────────────────────────────────────

/// `clone(flags, stack, ptid, ctid, tls)` → TID ou errno.
pub fn sys_clone(flags: u64, stack: u64, ptid: u64, ctid: u64, tls: u64, _a6: u64) -> i64 {
    if ptid != 0 && ptid >= USER_ADDR_MAX { return EFAULT; }
    if ctid != 0 && ctid >= USER_ADDR_MAX { return EFAULT; }
    // Point d'entrée : tls en priorité (pthread_create) puis ctid.
    let start_func = if tls != 0 { tls } else { ctid };
    if start_func == 0 { return EINVAL; }
    let stack_addr = if stack != 0 { stack.saturating_sub(16) } else { 0 };
    let stack_size = if stack != 0 { 0u64 } else { 8 * 1024 * 1024u64 };
    let detached   = (flags & 0x0040_0000) != 0;
    // Lecture PID courant depuis TCB per-CPU
    let current_pid_val: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 { return EFAULT; }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock)).pid.0
    };
    let pcb_ref = match crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(current_pid_val))
    {
        Some(p) => p,
        None    => return -3i64, // ESRCH
    };
    let attr = crate::process::thread::creation::ThreadAttr {
        stack_size,
        stack_addr,
        policy:           crate::scheduler::core::task::SchedPolicy::Normal,
        priority:         crate::scheduler::core::task::Priority::NORMAL_DEFAULT,
        detached,
        cpu_affinity:     -1,
        sigaltstack_size: 8192,
    };
    let params = crate::process::thread::creation::ThreadCreateParams {
        pcb:         pcb_ref as *const crate::process::core::pcb::ProcessControlBlock,
        attr,
        start_func,
        arg:         0,
        target_cpu:  0,
        pthread_out: ptid,
    };
    match crate::process::thread::creation::create_thread(&params) {
        Ok(handle)  => handle.tid.0 as i64,
        Err(crate::process::thread::creation::ThreadCreateError::OutOfMemory)  => ENOMEM,
        Err(crate::process::thread::creation::ThreadCreateError::TidExhausted) => EAGAIN,
        Err(_) => EINVAL,
    }
}
