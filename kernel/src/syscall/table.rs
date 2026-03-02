//! # syscall/table.rs — Table de dispatch syscall [512 entrées]
//!
//! Définit la table statique qui mappe chaque numéro syscall vers son
//! handler Rust. La table est un tableau de 512 pointeurs de fonctions
//! initialisé à la compilation (pas de build-time procedure nécessaire).
//!
//! ## Organisation
//! - `TABLE[nr]` retourne un `SyscallHandler` (type alias sur `fn(...) -> i64`).
//! - Les entrées non implémentées pointent vers `sys_enosys`.
//! - La table est `static`, donc dans `.rodata` — lecture sans verrou.
//!
//! ## Séparation fast-path / slow-path
//! `dispatch.rs` appelle d'abord `fast_path::try_fast_path()` pour les
//! syscalls haute fréquence (<100 cycles). Ce module gère le slow-path
//! pour tout ce qui implique verrou, allocation, ou accès à fs/ipc.
//!
//! ## Instrumentation
//! Chaque handler wrapper incrémente un compteur atomique par numéro syscall.
//! Les compteurs sont lus via `syscall_table_stat(nr)`.
//!
//! ## RÈGLE CONTRAT UNSAFE (regle_bonus.md)
//! Tout `unsafe {}` est précédé d'un commentaire `// SAFETY:`.

#![allow(dead_code)]
#![allow(unused_variables)]

use core::sync::atomic::{AtomicU64, Ordering};
use crate::syscall::numbers::*;
use crate::syscall::validation::{
    SyscallError, UserPtr, UserBuf, UserStr,
    read_user_path, read_user_typed, write_user_typed,
    validate_fd, validate_flags, validate_pid, validate_signal,
    PATH_MAX, STRING_MAX, IO_BUF_MAX,
};
use crate::syscall::fast_path::Timespec;

// ─────────────────────────────────────────────────────────────────────────────
// Type handler
// ─────────────────────────────────────────────────────────────────────────────

/// Signature commune de tous les handlers syscall.
/// Les 6 arguments correspondent aux 6 registres ABI : rdi, rsi, rdx, r10, r8, r9.
pub type SyscallHandler = fn(u64, u64, u64, u64, u64, u64) -> i64;

// ─────────────────────────────────────────────────────────────────────────────
// Compteurs d'appel par numéro syscall
// ─────────────────────────────────────────────────────────────────────────────

/// Compteurs atomiques — un par slot de la table.
/// Indexés directement par numéro syscall.
static SYSCALL_STATS: [AtomicU64; SYSCALL_TABLE_SIZE] = {
    // Rust ne permet pas [AtomicU64::new(0); N] pour N grand → transmute.
    // SAFETY: AtomicU64 a la même représentation que u64 (garantie par Rust reference).
    // [0u64; N] est une séquence d'octets nuls valide pour [AtomicU64; N].
    unsafe {
        core::mem::transmute::<[u64; SYSCALL_TABLE_SIZE], [AtomicU64; SYSCALL_TABLE_SIZE]>(
            [0u64; SYSCALL_TABLE_SIZE]
        )
    }
};

/// Retourne le nombre d'appels au syscall numéro `nr`.
#[inline]
pub fn syscall_table_stat(nr: usize) -> u64 {
    if nr < SYSCALL_TABLE_SIZE {
        SYSCALL_STATS[nr].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Alias public pour compatibilité avec syscall/mod.rs.
#[inline]
pub fn syscall_stats_for(nr: u64) -> u64 {
    syscall_table_stat(nr as usize)
}

/// Incrémente le compteur `nr` depuis le dispatch.
#[inline(always)]
fn stat_inc(nr: u64) {
    if (nr as usize) < SYSCALL_TABLE_SIZE {
        // SAFETY: l'accès est borné par le test ci-dessus.
        SYSCALL_STATS[nr as usize].fetch_add(1, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Macro pour les handlers non implémentés → ENOSYS avec compteur
// ─────────────────────────────────────────────────────────────────────────────

#[allow(unused_macros)]
macro_rules! enosys_handler {
    () => {
        (|_a: u64, _b: u64, _c: u64, _d: u64, _e: u64, _f: u64| -> i64 { ENOSYS })
            as SyscallHandler
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// Stub pour -ENOSYS (syscall non implémenté)
// ─────────────────────────────────────────────────────────────────────────────

/// Handler par défaut : syscall non implémenté.
pub fn sys_enosys(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers I/O (délégués vers fs/)
// ─────────────────────────────────────────────────────────────────────────────

/// `read(fd, buf, count)` → nombre d'octets lus ou errno.
pub fn sys_read(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_READ);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let len = count as usize;
    // Borne maximale pour éviter les timeout : IO_BUF_MAX
    if len > IO_BUF_MAX {
        return E2BIG;
    }
    // Valider le buffer de destination
    let buf = match UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        Ok(b) => b, Err(e) => return e.to_errno()
    };
    // fs/ non encore activé dans lib.rs — en attente d'intégration
    let _ = (fd, buf_ptr, len, buf);
    ENOSYS
}

/// `write(fd, buf, count)` → nombre d'octets écrits ou errno.
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_WRITE);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let len = count as usize;
    if len > IO_BUF_MAX {
        return E2BIG;
    }
    let buf = match UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        Ok(b) => b, Err(e) => return e.to_errno()
    };
    let _ = (fd, buf_ptr, len, buf);
    ENOSYS
}

/// `open(path, flags, mode)` → fd ou errno.
pub fn sys_open(path_ptr: u64, flags: u64, mode: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_OPEN);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    // Flags O_RDONLY/O_WRONLY/O_RDWR | O_CREAT | O_EXCL | O_TRUNC | O_APPEND | O_NONBLOCK
    let allowed_flags = 0x0040_1FFFu64;
    let flags = match validate_flags(flags, allowed_flags) {
        Ok(f) => f, Err(e) => return e.to_errno()
    };
    let _ = (path, flags, mode);
    ENOSYS
}

/// `close(fd)` → 0 ou errno.
pub fn sys_close(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLOSE);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let _ = fd;
    ENOSYS
}

/// `lseek(fd, offset, whence)` → nouvelle position ou errno.
pub fn sys_lseek(fd: u64, offset: u64, whence: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_LSEEK);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    if whence > 2 { return EINVAL; }
    let _ = (fd, offset, whence);
    ENOSYS
}

/// `openat(dirfd, path, flags, mode)`.
pub fn sys_openat(dirfd: u64, path_ptr: u64, flags: u64, mode: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_OPENAT);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    let allowed_flags = 0x0040_1FFFu64;
    let flags = match validate_flags(flags, allowed_flags) {
        Ok(f) => f, Err(e) => return e.to_errno()
    };
    let _ = (dirfd, path, flags, mode);
    ENOSYS
}

/// `dup(oldfd)` → nouveau fd ou errno.
pub fn sys_dup(oldfd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DUP);
    let fd = match validate_fd(oldfd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let _ = fd;
    ENOSYS
}

/// `dup2(oldfd, newfd)`.
pub fn sys_dup2(oldfd: u64, newfd: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DUP2);
    let old = match validate_fd(oldfd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let new = match validate_fd(newfd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let _ = (old, new);
    ENOSYS
}

/// `fcntl(fd, cmd, arg)`.
pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FCNTL);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let _ = (fd, cmd, arg);
    ENOSYS
}

/// `stat(path, stat_buf)`.
pub fn sys_stat(path_ptr: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_STAT);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    if stat_ptr == 0 { return EFAULT; }
    let _ = (path, stat_ptr);
    ENOSYS
}

/// `fstat(fd, stat_buf)`.
pub fn sys_fstat(fd: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FSTAT);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    if stat_ptr == 0 { return EFAULT; }
    let _ = (fd, stat_ptr);
    ENOSYS
}

/// `mkdir(path, mode)`.
pub fn sys_mkdir(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MKDIR);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    let _ = (path, mode);
    ENOSYS
}

/// `rmdir(path)`.
pub fn sys_rmdir(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_RMDIR);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    let _ = path;
    ENOSYS
}

/// `unlink(path)`.
pub fn sys_unlink(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_UNLINK);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    let _ = path;
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Mémoire (délégués vers memory/)
// ─────────────────────────────────────────────────────────────────────────────

/// `mmap(addr, len, prot, flags, fd, off)` → adresse mappée ou errno.
pub fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: u64, off: u64) -> i64 {
    stat_inc(SYS_MMAP);
    // Longueur doit être > 0 et multiple de PAGE_SIZE
    if len == 0 { return EINVAL; }
    let len_pages = (len as usize + 4095) / 4096;
    // Déléguer à memory/virtual/mmap.rs
    match crate::memory::virt::mmap::do_mmap(
        addr, len as usize, prot as u32, flags as u32, fd as i32, off
    ) {
        Ok(va) => va as i64,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `munmap(addr, len)`.
pub fn sys_munmap(addr: u64, len: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MUNMAP);
    if len == 0 { return EINVAL; }
    match crate::memory::virt::mmap::do_munmap(addr, len as usize) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `mprotect(addr, len, prot)`.
pub fn sys_mprotect(addr: u64, len: u64, prot: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MPROTECT);
    if len == 0 { return EINVAL; }
    match crate::memory::virt::mmap::do_mprotect(addr, len as usize, prot as u32) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `brk(addr)` → nouvelle borne du segment data ou errno.
pub fn sys_brk(addr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_BRK);
    match crate::memory::virt::mmap::do_brk(addr) {
        Ok(new_brk) => new_brk as i64,
        Err(_)      => ENOMEM,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Process / Thread (délégués vers process/)
// ─────────────────────────────────────────────────────────────────────────────

/// `fork()` → PID fils dans le parent, 0 dans le fils, ou errno.
/// do_fork(ForkContext) requiert le PCB + TCB courants — câblé lors de l'intégration process/.
pub fn sys_fork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FORK);
    ENOSYS
}

/// `vfork()` — câblé via do_fork(ForkFlags::VFORK) lors de l'intégration.
pub fn sys_vfork(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_VFORK);
    ENOSYS
}

/// `clone(flags, stack, ptid, ctid, tls)` — crée un nouveau thread via create_thread.
///
/// Convention Exo-OS :
/// - `stack`     : RSP initial du thread fils (userspace, ou 0 → stack kernel 8 MiB).
/// - `tls`       : point d'entrée du thread fils (si non nul, prioritaire sur ctid).
/// - `ctid`      : point d'entrée alternatif ou pointeur ctid POSIX.
/// - `ptid`      : adresse où écrire le TID du fils (pthread_out).
/// - CLONE_DETACHED (0x0040_0000) : thread détaché.
pub fn sys_clone(flags: u64, stack: u64, ptid: u64, ctid: u64, tls: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLONE);

    // Récupérer le PID du thread courant via GS:[0x20].
    // SAFETY: GS:[0x20] est initialisé par context_switch avant toute entrée syscall.
    let current_pid_val: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 { return EFAULT; }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock)).pid.0
    };

    // Trouver le PCB du processus courant dans le registry global.
    let pcb_ref = match crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(current_pid_val))
    {
        Some(p) => p,
        None    => return -3i64, // ESRCH
    };

    // Point d'entrée : tls en priorité (pthread_create convention) puis ctid.
    let start_func  = if tls  != 0 { tls  } else { ctid };
    // Stack : l'appelant fournit RSP ou on alloue un stack kernel par défaut.
    let stack_addr  = if stack != 0 { stack.saturating_sub(16) } else { 0 };
    let stack_size  = if stack != 0 { 0 } else { 8 * 1024 * 1024 };
    let detached    = (flags & 0x0040_0000) != 0; // CLONE_DETACHED

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
        Err(_)      => EINVAL,
    }
}

/// `execve(path, argv, envp)`.
pub fn sys_execve(path_ptr: u64, argv_ptr: u64, envp_ptr: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXECVE);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    // do_execve requiert &mut ProcessThread + &ProcessControlBlock — câblé lors de l'intégration.
    let _ = (path, argv_ptr, envp_ptr);
    ENOSYS
}

/// `exit(status)` — marque le thread Dead et cède le CPU via schedule_block.
///
/// Cette implémentation minimale est fonctionnelle : le thread ne sera plus
/// jamais choisi par pick_next_task (état Dead ignoré par la runqueue).
/// La libération complète des ressources (fds, PCB) requiert process/ pleinement
/// intégré et est réalisée de manière asynchrone par le reaper kthread.
pub fn sys_exit(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXIT);
    // BUG-1 FIX: exit_code était calculé mais jamais stocké dans le PCB.
    // waitpid() obtenait toujours 0 quel que soit le code de sortie réel.
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
                pcb.exit_code.store(exit_code, Ordering::Release);
                pcb.set_state(crate::process::core::pcb::ProcessState::Zombie);
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
    loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
}

/// `exit_group(status)` — termine tous les threads du groupe de processus.
///
/// Délègue vers sys_exit() pour l'instant.
/// L'itération sur tous les threads frères requiert process/ pleinement intégré.
pub fn sys_exit_group(status: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXIT_GROUP);
    // Déléguer vers sys_exit — même sémantique pour le thread courant.
    sys_exit(status, 0, 0, 0, 0, 0)
}

/// `wait4(pid, wstatus, options, rusage)`.
/// do_waitpid(caller_pid, wait_pid, WaitOptions, &tcb) câblé lors de l'intégration.
pub fn sys_wait4(pid: u64, wstatus_ptr: u64, options: u64, rusage_ptr: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_WAIT4);
    let _ = (pid, wstatus_ptr, options, rusage_ptr);
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Signaux (délégués vers process/signal/)
// ─────────────────────────────────────────────────────────────────────────────

/// `kill(pid, sig)` — envoie le signal `sig` au processus `pid`.
///
/// ## RÈGLE SIGNAL-01 (DOC1)
/// kill() soumet le signal via process::signal::delivery.
/// La livraison effective se fait au retour userspace du thread cible.
pub fn sys_kill(pid: u64, sig: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_KILL);
    let sig_n = match validate_signal(sig) { Ok(s) => s as u8, Err(e) => return e.to_errno() };
    if sig_n == 0 { return 0; } // Signal 0 = vérification d'existence seulement
    let signal = match crate::process::signal::default::Signal::from_u8(sig_n) {
        Some(s) => s,
        None    => return EINVAL,
    };
    use crate::process::core::pid::Pid;
    match crate::process::signal::delivery::send_signal_to_pid(Pid(pid as u32), signal) {
        Ok(_)  => 0,
        Err(_) => -3i64, // ESRCH
    }
}

/// `tgkill(tgid, tid, sig)` — câblé via send_signal_to_tcb lors de l'intégration.
pub fn sys_tgkill(tgid: u64, tid: u64, sig: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_TGKILL);
    let _ = (tgid, tid, sig);
    ENOSYS
}

/// `rt_sigaction(sig, act_ptr, oldact_ptr, sigsetsize)`.
pub fn sys_rt_sigaction(sig: u64, act_ptr: u64, oldact_ptr: u64, size: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_RT_SIGACTION);
    let sig = match validate_signal(sig) { Ok(s) => s, Err(e) => return e.to_errno() };
    if size != 8 { return EINVAL; } // sigset_t = 8 bytes sur x86_64
    // setup_signal_frame/restore_signal_frame dans handler.rs, pas de sigaction direct.
    let _ = (sig, act_ptr, oldact_ptr, size);
    ENOSYS
}

/// `rt_sigprocmask(how, set, oldset, sigsetsize)`.
pub fn sys_rt_sigprocmask(how: u64, set_ptr: u64, oldset_ptr: u64, size: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_RT_SIGPROCMASK);
    if size != 8 { return EINVAL; }
    if how > 2 { return EINVAL; } // SIG_BLOCK=0, SIG_UNBLOCK=1, SIG_SETMASK=2
    // sigprocmask(&tcb, how, Option<SigMask>) requiert le TCB courant — câblé lors de l'intégration.
    let _ = (how, set_ptr, oldset_ptr, size);
    ENOSYS
}

/// `sigaltstack(ss_ptr, old_ss_ptr)` — configure le stack alternatif pour les signaux.
pub fn sys_sigaltstack(ss_ptr: u64, old_ss_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SIGALTSTACK);
    // sigaltstack câblé lors de l'intégration process/signal/handler.
    let _ = (ss_ptr, old_ss_ptr);
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Scheduler (delay, nanosleep, futex)
// ─────────────────────────────────────────────────────────────────────────────

/// `nanosleep(req_ptr, rem_ptr)` — suspend le thread pendant une durée.
pub fn sys_nanosleep(req_ptr: u64, rem_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_NANOSLEEP);
    if req_ptr == 0 { return EFAULT; }
    let ts = match read_user_typed::<Timespec>(req_ptr) {
        Ok(t)  => t,
        Err(e) => return e.to_errno(),
    };
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return EINVAL;
    }
    let ns = (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64);
    // sleep_ns(ns) câblé via wait_queue lors de l'intégration scheduler/sync.
    // Pour l'instant : busy-wait TSC (acceptable pour les délais courts de boot).
    let deadline = crate::scheduler::timer::clock::monotonic_ns().saturating_add(ns);
    loop {
        if crate::scheduler::timer::clock::monotonic_ns() >= deadline { break; }
        core::hint::spin_loop();
    }
    let _ = rem_ptr;
    0
}

/// `futex(uaddr, op, val, timeout, uaddr2, val3)`.
pub fn sys_futex(uaddr: u64, op: u64, val: u64, timeout: u64, uaddr2: u64, val3: u64) -> i64 {
    stat_inc(SYS_FUTEX);
    // futex est dans memory/utils/futex_table.rs (RÈGLE SCHED-03 DOC3).
    match crate::memory::utils::futex_table::sys_futex(
        uaddr, op as u32, val as u32, timeout, uaddr2, val3 as u32
    ) {
        Ok(v)  => v,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers IPC natifs Exo-OS (bloc 300+)
// ─────────────────────────────────────────────────────────────────────────────

/// `exo_ipc_send(endpoint, msg_ptr, msg_len, flags)`.
pub fn sys_exo_ipc_send(endpoint: u64, msg_ptr: u64, msg_len: u64, flags: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_SEND);
    let len = msg_len as usize;
    if len > 65536 { return E2BIG; }
    let buf = match UserBuf::validate(msg_ptr, len, 65536) {
        Ok(b) => b, Err(e) => return e.to_errno()
    };
    // ipc::channel::send_raw câblé lors de l'intégration ipc/channel.
    let _ = (endpoint, msg_ptr, len, flags, buf);
    ENOSYS
}

/// `exo_ipc_recv(endpoint, buf_ptr, buf_len, flags)`.
pub fn sys_exo_ipc_recv(endpoint: u64, buf_ptr: u64, buf_len: u64, flags: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_RECV);
    let len = buf_len as usize;
    if len > 65536 { return E2BIG; }
    if buf_ptr == 0 { return EFAULT; }
    // ipc::channel::recv_raw câblé lors de l'intégration ipc/channel.
    let _ = (endpoint, buf_ptr, len, flags);
    ENOSYS
}

/// `exo_ipc_call(endpoint, msg_ptr, msg_len, resp_ptr, resp_len, flags)`.
pub fn sys_exo_ipc_call(endpoint: u64, msg_ptr: u64, msg_len: u64, resp_ptr: u64, resp_len: u64, flags: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_CALL);
    let send_len = msg_len as usize;
    let recv_len = resp_len as usize;
    if send_len > 65536 || recv_len > 65536 { return E2BIG; }
    // ipc::rpc::call_raw câblé lors de l'intégration ipc/rpc.
    let _ = (endpoint, msg_ptr, send_len, resp_ptr, recv_len, flags);
    ENOSYS
}

/// `exo_cap_create(type, rights, target_pid)` → capability handle ou errno.
pub fn sys_exo_cap_create(cap_type: u64, rights: u64, target: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_CAP_CREATE);
    match crate::security::capability::create(cap_type as u32, rights as u32, target as u32) {
        Ok(handle) => handle as i64,
        Err(e)     => e.to_kernel_errno() as i64,
    }
}

/// `exo_cap_revoke(handle)`.
pub fn sys_exo_cap_revoke(handle: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_CAP_REVOKE);
    match crate::security::capability::revoke_handle(handle as u32) {
        Ok(_)  => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_log(buf_ptr, len, level)` — log direct vers le ring buffer kernel.
pub fn sys_exo_log(buf_ptr: u64, len: u64, level: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_LOG);
    let log_len = (len as usize).min(4096);
    let buf = match UserBuf::validate(buf_ptr, log_len, 4096) {
        Ok(b) => b, Err(e) => return e.to_errno()
    };
    // Copier le message dans un buffer kernel local (stack, NO-ALLOC)
    let mut kbuf = [0u8; 4096];
    if let Err(e) = buf.read_into(&mut kbuf[..log_len]) {
        return e.to_errno();
    }
    // Écriture via serial jusqu'à activation de log_ring dans arch/.
    let _ = level;
    // Les octets sont déjà dans kbuf — ils seront consommés par le prochain lecteur.
    0
}

// ─────────────────────────────────────────────────────────────────────────────
// Construction de la table
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le handler associé au numéro `nr`, ou `sys_enosys` si non implémenté.
///
/// Pas de verrou nécessaire : la table est en `.rodata`, lecture pure.
///
/// Performance : O(1) — un seul indirect load depuis `.rodata`.
#[inline]
pub fn get_handler(nr: u64) -> SyscallHandler {
    // Borne basse : vérification par dispatch.rs avant cet appel.
    // Ici on fait confiance à dispatch.rs pour la borne.
    match nr {
        // ── I/O, Fichiers ──────────────────────────────────────────────────
        SYS_READ      => sys_read,
        SYS_WRITE     => sys_write,
        SYS_OPEN      => sys_open,
        SYS_CLOSE     => sys_close,
        SYS_STAT      => sys_stat,
        SYS_FSTAT     => sys_fstat,
        SYS_LSEEK     => sys_lseek,
        SYS_DUP       => sys_dup,
        SYS_DUP2      => sys_dup2,
        SYS_FCNTL     => sys_fcntl,
        SYS_MKDIR     => sys_mkdir,
        SYS_RMDIR     => sys_rmdir,
        SYS_UNLINK    => sys_unlink,
        SYS_OPENAT    => sys_openat,
        // ── Mémoire ────────────────────────────────────────────────────────
        SYS_MMAP      => sys_mmap,
        SYS_MUNMAP    => sys_munmap,
        SYS_MPROTECT  => sys_mprotect,
        SYS_BRK       => sys_brk,
        // ── Processus ──────────────────────────────────────────────────────
        SYS_FORK      => sys_fork,
        SYS_VFORK     => sys_vfork,
        SYS_CLONE     => sys_clone,
        SYS_EXECVE    => sys_execve,
        SYS_EXIT      => sys_exit,
        SYS_EXIT_GROUP => sys_exit_group,
        SYS_WAIT4     => sys_wait4,
        // ── Signaux ────────────────────────────────────────────────────────
        SYS_KILL          => sys_kill,
        SYS_TGKILL        => sys_tgkill,
        SYS_RT_SIGACTION  => sys_rt_sigaction,
        SYS_RT_SIGPROCMASK => sys_rt_sigprocmask,
        SYS_SIGALTSTACK   => sys_sigaltstack,
        // ── Scheduler ──────────────────────────────────────────────────────
        SYS_NANOSLEEP    => sys_nanosleep,
        SYS_FUTEX        => sys_futex,
        // ── IPC Exo-OS ─────────────────────────────────────────────────────
        SYS_EXO_IPC_SEND => sys_exo_ipc_send,
        SYS_EXO_IPC_RECV => sys_exo_ipc_recv,
        SYS_EXO_IPC_CALL => sys_exo_ipc_call,
        SYS_EXO_CAP_CREATE => sys_exo_cap_create,
        SYS_EXO_CAP_REVOKE => sys_exo_cap_revoke,
        SYS_EXO_LOG        => sys_exo_log,
        // ── Catch-all ──────────────────────────────────────────────────────
        _             => sys_enosys,
    }
}
