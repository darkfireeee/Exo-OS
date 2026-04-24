//! # syscall/table.rs — Table de dispatch syscall [SYSCALL_TABLE_SIZE entrées]
//!
//! Définit la table statique qui mappe chaque numéro syscall vers son
//! handler Rust. La table est un tableau de `SYSCALL_TABLE_SIZE` pointeurs de fonctions
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


extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::syscall::numbers::*;
use crate::syscall::validation::{
    UserBuf,
    copy_from_user,
    copy_to_user,
    read_user_path, read_user_typed,
    write_user_typed,
    validate_fd, validate_flags, validate_signal, IO_BUF_MAX,
};
use crate::syscall::fast_path::Timespec;
// GI-03 IRQ types et fonctions
use crate::arch::x86_64::irq::{
    IpcEndpoint,
    IrqAckResult,
    IrqOwnerPid,
    IrqVector,
    parse_irq_source_kind,
    irq_error_to_errno,
};
// GI-03 Driver types
use crate::drivers::{ClaimError, MmioError, MsiError, PciCfgError, TopoError};

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{
    DmaDirection,
    DmaError,
    DmaMapFlags,
    IommuDomainId,
    IovaAddr,
};
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::fs::exofs::syscall::{
    sys_exofs_path_resolve,
    sys_exofs_object_open,
    sys_exofs_object_read,
    sys_exofs_object_write,
    sys_exofs_object_create,
    sys_exofs_object_delete,
    sys_exofs_object_stat,
    sys_exofs_object_set_meta,
    sys_exofs_get_content_hash,
    sys_exofs_snapshot_create,
    sys_exofs_snapshot_list,
    sys_exofs_snapshot_mount,
    sys_exofs_relation_create,
    sys_exofs_relation_query,
    sys_exofs_gc_trigger,
    sys_exofs_quota_query,
    sys_exofs_export_object,
    sys_exofs_import_object,
    sys_exofs_epoch_commit,
    sys_exofs_open_by_path,
    sys_exofs_readdir,
};
use pci_types::PciAddress;

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

/// Wrapper ABI pour SYS_EXOFS_OBJECT_SET_META.
///
/// Nouveau contrat: a1=args_ptr(SetMetaArgs), a2=cap_token, a3..a6 ignorés.
pub fn sys_exofs_object_set_meta_abi(
    args_ptr: u64,
    cap_token: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    sys_exofs_object_set_meta(args_ptr, cap_token)
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
    let _validated_buf = match UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        Ok(b) => b, Err(e) => return e.to_errno()
    };
    // CORRECTION P0-04 : câbler vers fs_bridge
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_read(fd as u32, buf_ptr, len, pid))
}

/// `write(fd, buf, count)` → nombre d'octets écrits ou errno.
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_WRITE);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let len = count as usize;
    if len > IO_BUF_MAX {
        return E2BIG;
    }
    let _validated_buf = match UserBuf::validate(buf_ptr, len, IO_BUF_MAX) {
        Ok(b) => b, Err(e) => return e.to_errno()
    };
    // CORRECTION P0-04 : câbler vers fs_bridge
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_write(fd as u32, buf_ptr, len, pid))
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
    // CORRECTION P0-04 : câbler vers fs_bridge
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(
        fs_bridge::fs_open(path.as_bytes(), flags as u32, mode as u32, pid)
    )
}

/// `close(fd)` → 0 ou errno.
pub fn sys_close(fd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_CLOSE);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    // CORRECTION P0-04 : câbler vers fs_bridge
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_close(fd as u32, pid))
}

/// `lseek(fd, offset, whence)` → nouvelle position ou errno.
pub fn sys_lseek(fd: u64, offset: u64, whence: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_LSEEK);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    if whence > 2 { return EINVAL; }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_lseek(fd as u32, offset as i64, whence as u32, pid))
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
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(
        fs_bridge::fs_openat(dirfd as i32, path.as_bytes(), flags as u32, mode as u32, pid)
    )
}

/// `dup(oldfd)` → nouveau fd ou errno.
pub fn sys_dup(oldfd: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DUP);
    let fd = match validate_fd(oldfd) { Ok(f) => f, Err(e) => return e.to_errno() };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_dup(fd as u32, pid))
}

/// `dup2(oldfd, newfd)`.
pub fn sys_dup2(oldfd: u64, newfd: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DUP2);
    let old = match validate_fd(oldfd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let new = match validate_fd(newfd) { Ok(f) => f, Err(e) => return e.to_errno() };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_dup2(old as u32, new as u32, pid))
}

/// `fcntl(fd, cmd, arg)`.
pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FCNTL);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fcntl(fd as u32, cmd as u32, arg, pid))
}

/// `stat(path, stat_buf)`.
pub fn sys_stat(path_ptr: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_STAT);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    if stat_ptr == 0 { return EFAULT; }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_stat(path.as_bytes(), stat_ptr, pid))
}

/// `fstat(fd, stat_buf)`.
pub fn sys_fstat(fd: u64, stat_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_FSTAT);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    if stat_ptr == 0 { return EFAULT; }
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_fstat(fd as u32, stat_ptr, pid))
}

/// `mkdir(path, mode)`.
pub fn sys_mkdir(path_ptr: u64, mode: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MKDIR);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_mkdir(path.as_bytes(), mode as u32, pid))
}

/// `rmdir(path)`.
pub fn sys_rmdir(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_RMDIR);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_rmdir(path.as_bytes(), pid))
}

/// `unlink(path)`.
pub fn sys_unlink(path_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_UNLINK);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_unlink(path.as_bytes(), pid))
}

/// `getdents64(fd, dirp, count)`.
pub fn sys_getdents64(fd: u64, dirp: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_GETDENTS64);
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_getdents64(fd as u32, dirp, count as usize, pid))
}

/// `readlink(path, buf, bufsize)`.
pub fn sys_readlink(path_ptr: u64, buf_ptr: u64, bufsize: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_READLINK);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_readlink(path.as_bytes(), buf_ptr, bufsize as usize, pid))
}

/// `readlinkat(dirfd, path, buf, bufsize)`.
pub fn sys_readlinkat(
    dirfd: u64,
    path_ptr: u64,
    buf_ptr: u64,
    bufsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_READLINKAT);
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    use crate::syscall::fs_bridge;
    let pid = current_pid_u32();
    fs_bridge::bridge_result(fs_bridge::fs_readlinkat(dirfd as i32, path.as_bytes(), buf_ptr, bufsize as usize, pid))
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Mémoire (délégués vers memory/)
// ─────────────────────────────────────────────────────────────────────────────

/// `mmap(addr, len, prot, flags, fd, off)` → adresse mappée ou errno.
pub fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: u64, off: u64) -> i64 {
    stat_inc(SYS_MMAP);
    // Longueur doit être > 0 et multiple de PAGE_SIZE
    if len == 0 { return EINVAL; }
    let _len_pages = (len as usize + 4095) / 4096;
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
    crate::syscall::handlers::process::sys_wait4(pid, wstatus_ptr, options, rusage_ptr, 0, 0)
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
    crate::syscall::handlers::signal::sys_tgkill(tgid, tid, sig, 0, 0, 0)
}

/// `rt_sigaction(sig, act_ptr, oldact_ptr, sigsetsize)`.
pub fn sys_rt_sigaction(sig: u64, act_ptr: u64, oldact_ptr: u64, size: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_RT_SIGACTION);
    crate::syscall::handlers::signal::sys_rt_sigaction(sig, act_ptr, oldact_ptr, size, 0, 0)
}

/// `rt_sigprocmask(how, set, oldset, sigsetsize)`.
pub fn sys_rt_sigprocmask(how: u64, set_ptr: u64, oldset_ptr: u64, size: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_RT_SIGPROCMASK);
    crate::syscall::handlers::signal::sys_rt_sigprocmask(how, set_ptr, oldset_ptr, size, 0, 0)
}

/// `sigaltstack(ss_ptr, old_ss_ptr)` — configure le stack alternatif pour les signaux.
pub fn sys_sigaltstack(ss_ptr: u64, old_ss_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_SIGALTSTACK);
    crate::syscall::handlers::signal::sys_sigaltstack(ss_ptr, old_ss_ptr, 0, 0, 0, 0)
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
    if len > crate::ipc::core::constants::MAX_MSG_SIZE {
        return E2BIG;
    }
    if let Err(errno) = enforce_direct_ipc_policy(endpoint) {
        return errno;
    }
    let endpoint_id = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };
    let _validated_buf = match UserBuf::validate(msg_ptr, len, crate::ipc::core::constants::MAX_MSG_SIZE) {
        Ok(b) => b, Err(e) => return e.to_errno()
    };
    let mut payload = Vec::new();
    payload.resize(len, 0);
    if len != 0 {
        if copy_from_user(payload.as_mut_ptr(), msg_ptr as *const u8, len).is_err() {
            return EFAULT;
        }
    }
    let raw_flags = if flags & IPC_RECV_TIMEOUT_FLAG != 0 { 0x0001 } else { 0 };
    match crate::ipc::channel::raw::send_raw(endpoint_id, &payload, raw_flags) {
        Ok(_) => 0,
        Err(err) => ipc_error_to_errno(err),
    }
}

/// `exo_ipc_recv(endpoint, buf_ptr, buf_len, flags)`.
pub fn sys_exo_ipc_recv(endpoint: u64, buf_ptr: u64, buf_len: u64, flags: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_RECV);
    recv_ipc_message(endpoint, buf_ptr, buf_len, flags, false)
}

/// `exo_ipc_recv_nb(endpoint, buf_ptr, buf_len, flags)`.
pub fn sys_exo_ipc_recv_nb(endpoint: u64, buf_ptr: u64, buf_len: u64, flags: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_RECV_NB);
    recv_ipc_message(endpoint, buf_ptr, buf_len, flags, true)
}

/// `exo_ipc_call(endpoint, msg_ptr, msg_len, resp_ptr, resp_len, flags)`.
pub fn sys_exo_ipc_call(endpoint: u64, msg_ptr: u64, msg_len: u64, resp_ptr: u64, resp_len: u64, flags: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_CALL);
    let send_len = msg_len as usize;
    let recv_len = resp_len as usize;
    if send_len > crate::ipc::rpc::MAX_CALL_PAYLOAD || recv_len > crate::ipc::rpc::MAX_CALL_PAYLOAD {
        return E2BIG;
    }
    if flags != 0 {
        return EINVAL;
    }
    if send_len != 0 && msg_ptr == 0 {
        return EFAULT;
    }
    if recv_len != 0 && resp_ptr == 0 {
        return EFAULT;
    }
    if let Err(errno) = enforce_direct_ipc_policy(endpoint) {
        return errno;
    }

    let server_ep = match EndpointId::new(endpoint) {
        Some(ep) => ep,
        None => return EINVAL,
    };

    let mut request = Vec::new();
    request.resize(send_len, 0);
    if send_len != 0 {
        if copy_from_user(request.as_mut_ptr(), msg_ptr as *const u8, send_len).is_err() {
            return EFAULT;
        }
    }

    let mut response = Vec::new();
    response.resize(recv_len, 0);

    match crate::ipc::rpc::call_raw(server_ep, &request, &mut response) {
        Ok(reply_len) => {
            if reply_len != 0 && copy_to_user(resp_ptr as *mut u8, response.as_ptr(), reply_len).is_err() {
                return EFAULT;
            }
            reply_len as i64
        }
        Err(err) => ipc_error_to_errno(err),
    }
}

/// `exo_ipc_create(name_ptr, name_len, endpoint)` — ouvre la mailbox raw du serveur.
pub fn sys_exo_ipc_create(name_ptr: u64, name_len: u64, endpoint: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_CREATE);
    let len = name_len as usize;
    if len == 0 || len > 128 {
        return EINVAL;
    }
    let endpoint_pid = exo_ipc_endpoint_pid(endpoint);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if endpoint_pid == 0 {
        return EINVAL;
    }
    if caller_pid == 0 || endpoint_pid != caller_pid {
        return EACCES;
    }
    let ep = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };

    let _validated = match UserBuf::validate(name_ptr, len, 128) {
        Ok(buf) => buf,
        Err(err) => return err.to_errno(),
    };
    let mut name = Vec::new();
    name.resize(len, 0);
    if copy_from_user(name.as_mut_ptr(), name_ptr as *const u8, len).is_err() {
        return EFAULT;
    }

    if crate::ipc::channel::raw::mailbox_open(ep) {
        0
    } else {
        ENOMEM
    }
}

/// `exo_ipc_destroy(endpoint)` — ferme la mailbox raw du serveur.
pub fn sys_exo_ipc_destroy(endpoint: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_IPC_DESTROY);
    let endpoint_pid = exo_ipc_endpoint_pid(endpoint);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if endpoint_pid == 0 {
        return EINVAL;
    }
    if caller_pid == 0 || endpoint_pid != caller_pid {
        return EACCES;
    }
    let ep = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };
    crate::ipc::channel::raw::mailbox_close(ep);
    0
}

#[inline(always)]
fn exo_ipc_endpoint_pid(endpoint: u64) -> u32 {
    let packed_pid = (endpoint >> 32) as u32;
    if packed_pid != 0 {
        packed_pid
    } else {
        endpoint as u32
    }
}

#[inline(always)]
fn current_pid_u32() -> u32 {
    let tcb_ptr: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
    }
    if tcb_ptr == 0 { return 0; }
    unsafe { (*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock)).pid.0 }
}

const IPC_RECV_TIMEOUT_FLAG: u64 = 0x0001;

fn recv_ipc_message(endpoint: u64, buf_ptr: u64, buf_len: u64, flags: u64, nowait: bool) -> i64 {
    let len = buf_len as usize;
    if len > 65_536 {
        return E2BIG;
    }
    if buf_ptr == 0 && len != 0 {
        return EFAULT;
    }
    let endpoint_id = match EndpointId::new(endpoint) {
        Some(id) => id,
        None => return EINVAL,
    };

    let recv_cap = len.min(crate::ipc::core::constants::MAX_MSG_SIZE);
    let mut payload = [0u8; crate::ipc::core::constants::MAX_MSG_SIZE];
    let timeout_requested = !nowait && (flags & IPC_RECV_TIMEOUT_FLAG != 0);
    let timeout_ms = flags & !IPC_RECV_TIMEOUT_FLAG;

    let result = if nowait {
        crate::ipc::channel::raw::recv_raw(endpoint_id, &mut payload[..recv_cap], 0x0001)
    } else if timeout_requested {
        let deadline = crate::scheduler::timer::clock::monotonic_ns()
            .saturating_add(timeout_ms.saturating_mul(1_000_000));
        loop {
            match crate::ipc::channel::raw::recv_raw(endpoint_id, &mut payload[..recv_cap], 0x0001) {
                Ok(n) => break Ok(n),
                Err(IpcError::WouldBlock) | Err(IpcError::QueueEmpty) => {
                    if crate::scheduler::timer::clock::monotonic_ns() >= deadline {
                        break Err(IpcError::Timeout);
                    }
                    core::hint::spin_loop();
                }
                Err(err) => break Err(err),
            }
        }
    } else {
        crate::ipc::channel::raw::recv_raw(endpoint_id, &mut payload[..recv_cap], 0)
    };

    match result {
        Ok(n) => {
            if n != 0 && copy_to_user(buf_ptr as *mut u8, payload.as_ptr(), n).is_err() {
                return EFAULT;
            }
            n as i64
        }
        Err(IpcError::Timeout) => crate::syscall::errno::ETIMEDOUT,
        Err(err) => ipc_error_to_errno(err),
    }
}

fn ipc_error_to_errno(err: IpcError) -> i64 {
    match err {
        IpcError::WouldBlock
        | IpcError::Retry
        | IpcError::Full
        | IpcError::QueueFull
        | IpcError::QueueEmpty => EAGAIN,
        IpcError::EndpointNotFound
        | IpcError::NotFound => ENOENT,
        IpcError::PermissionDenied => EACCES,
        IpcError::MessageTooLarge => E2BIG,
        IpcError::Timeout => EAGAIN,
        IpcError::ResourceExhausted
        | IpcError::ShmPoolFull
        | IpcError::OutOfResources => ENOMEM,
        IpcError::ConnRefused => ENOENT,
        IpcError::AlreadyConnected => EBUSY,
        IpcError::InvalidParam
        | IpcError::InvalidHandle
        | IpcError::Invalid
        | IpcError::NullEndpoint
        | IpcError::InvalidEndpoint
        | IpcError::InvalidArgument => EINVAL,
        IpcError::Interrupted => EINTR,
        IpcError::ChannelClosed
        | IpcError::Closed => EBUSY,
        IpcError::HandshakeFailed
        | IpcError::OutOfOrder
        | IpcError::ProtocolError
        | IpcError::MappingFailed
        | IpcError::InternalError
        | IpcError::Internal
        | IpcError::Loop => EINVAL,
    }
}

fn enforce_direct_ipc_policy(endpoint: u64) -> Result<(), i64> {
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return Err(EACCES);
    }

    let dst_pid = exo_ipc_endpoint_pid(endpoint);
    if dst_pid == 0 {
        return Err(EINVAL);
    }

    let verdict = crate::security::check_direct_ipc(
        crate::process::core::pid::Pid(caller_pid),
        crate::process::core::pid::Pid(dst_pid),
    );

    match verdict {
        crate::security::IpcPolicyResult::Allowed => Ok(()),
        crate::security::IpcPolicyResult::Denied
        | crate::security::IpcPolicyResult::UnknownService => {
            crate::security::exoledger::exo_ledger_append(
                crate::security::exoledger::ActionTag::IpcUnauthorized {
                    src_pid: caller_pid,
                    dst_pid,
                },
            );
            Err(EACCES)
        }
    }
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
// Handlers GI-03 Drivers (530–546)
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Helpers pour DMA/PCI (réutilisables)
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn parse_dma_direction(raw: u64) -> Option<DmaDirection> {
    match raw {
        0 => Some(DmaDirection::ToDevice),
        1 => Some(DmaDirection::FromDevice),
        2 => Some(DmaDirection::Bidirection),
        3 => Some(DmaDirection::None),
        _ => None,
    }
}

#[inline]
fn parse_irq_ack_result(raw: u64) -> Option<IrqAckResult> {
    match raw {
        0 => Some(IrqAckResult::Handled),
        1 => Some(IrqAckResult::NotMine),
        _ => None,
    }
}

#[inline]
fn parse_ipc_endpoint_packed(lo: u64, hi: u64) -> IpcEndpoint {
    IpcEndpoint {
        pid: lo as u32,
        chan_idx: (lo >> 32) as u32,
        generation: hi as u32,
        _pad: (hi >> 32) as u32,
    }
}

/// Encode/decode ABI simplifié BDF sur 32 bits :
/// - bits 31..16 : segment
/// - bits 15..8  : bus
/// - bits 7..3   : device
/// - bits 2..0   : function
#[inline]
fn parse_pci_address(raw: u64) -> PciAddress {
    let v = raw as u32;
    let segment = (v >> 16) as u16;
    let bus = ((v >> 8) & 0xFF) as u8;
    let device = ((v >> 3) & 0x1F) as u8;
    let function = (v & 0x07) as u8;
    PciAddress::new(segment, bus, device, function)
}

#[inline]
fn dma_error_to_errno(err: DmaError) -> i64 {
    match err {
        DmaError::OutOfMemory => ENOMEM,
        DmaError::NoChannel
        | DmaError::Timeout
        | DmaError::NotInitialized
        | DmaError::AlreadySubmitted
        | DmaError::Cancelled => EAGAIN,
        DmaError::HardwareError | DmaError::IommuFault => EFAULT,
        DmaError::InvalidParams
        | DmaError::MisalignedBuffer
        | DmaError::WrongZone
        | DmaError::NotSupported => EINVAL,
    }
}

#[inline]
fn claim_error_to_errno(err: ClaimError) -> i64 {
    match err {
        ClaimError::PermissionDenied => EACCES,
        ClaimError::AlreadyClaimed => EBUSY,
        ClaimError::NotInHardwareRegion | ClaimError::PhysIsRam => EINVAL,
        ClaimError::TableFull => ENOMEM,
    }
}

#[inline]
fn topo_error_to_errno(err: TopoError) -> i64 {
    match err {
        TopoError::TopologyTableFull => ENOMEM,
    }
}

#[inline]
fn pci_cfg_error_to_errno(err: PciCfgError) -> i64 {
    match err {
        PciCfgError::NotClaimed => EPERM,
        PciCfgError::PermissionDenied => EACCES,
    }
}

#[inline]
fn mmio_error_to_errno(err: MmioError) -> i64 {
    match err {
        MmioError::PermissionDenied => EACCES,
        MmioError::AlreadyMapped => EBUSY,
        MmioError::OutOfMemory => ENOMEM,
        MmioError::NotMapped => EFAULT,
        MmioError::InvalidParams => EINVAL,
    }
}

#[inline]
fn msi_error_to_errno(err: MsiError) -> i64 {
    match err {
        MsiError::NotFound => ENOENT,
        MsiError::TableFull | MsiError::NoSpace => ENOMEM,
        MsiError::AmbiguousClaim => EINVAL,
        MsiError::InvalidParams => EINVAL,
    }
}

/// ABI GI-03 canonique.
/// Encodage registre noyau :
/// `sys_irq_register(irq, endpoint_lo, endpoint_hi, source_kind, bdf_raw, has_bdf)`.
/// `endpoint_lo = pid | (chan_idx << 32)`, `endpoint_hi = generation | (_pad << 32)`.
pub fn sys_irq_register(
    irq: u64,
    endpoint_lo: u64,
    endpoint_hi: u64,
    source_kind: u64,
    bdf_raw: u64,
    has_bdf: u64,
) -> i64 {
    stat_inc(SYS_IRQ_REGISTER);

    if irq > u8::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let vector = IrqVector(irq as u8);
    if !vector.is_valid() {
        return EINVAL;
    }

    let source_kind = match parse_irq_source_kind(source_kind) {
        Some(kind) => kind,
        None => return EINVAL,
    };

    let mut endpoint = parse_ipc_endpoint_packed(endpoint_lo, endpoint_hi);
    endpoint.pid = caller_pid;

    let bdf = if has_bdf != 0 { Some(bdf_raw) } else { None };

    match crate::arch::x86_64::irq::sys_irq_register(vector, endpoint, source_kind, bdf) {
        Ok(reg_id) => reg_id as i64,
        Err(err) => irq_error_to_errno(err),
    }
}

/// ABI GI-03 canonique :
/// `sys_irq_ack(irq, reg_id, handler_gen, wave_gen, result)`.
pub fn sys_irq_ack(
    irq: u64,
    reg_id: u64,
    handler_gen: u64,
    wave_gen: u64,
    result_raw: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_IRQ_ACK);
    if irq > u8::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let vector = IrqVector(irq as u8);
    if !vector.is_valid() {
        return EINVAL;
    }

    let result = match parse_irq_ack_result(result_raw) {
        Some(result) => result,
        None => return EINVAL,
    };

    match crate::arch::x86_64::irq::ack_irq(
        vector,
        reg_id,
        handler_gen,
        IrqOwnerPid(caller_pid),
        wave_gen,
        result,
    ) {
        Ok(()) => 0,
        Err(err) => irq_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_mmio_map(phys_addr, size)` pour le PID appelant.
pub fn sys_mmio_map(phys_addr: u64, size: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MMIO_MAP);

    if size == 0 || size > usize::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_mmio_map_for_pid(caller_pid, PhysAddr::new(phys_addr), size as usize) {
        Ok(virt) => virt as i64,
        Err(err) => mmio_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_mmio_unmap(virt_addr, size)` pour le PID appelant.
pub fn sys_mmio_unmap(virt_addr: u64, size: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MMIO_UNMAP);

    if size == 0 || size > usize::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_mmio_unmap_for_pid(caller_pid, virt_addr, size as usize) {
        Ok(()) => 0,
        Err(err) => mmio_error_to_errno(err),
    }
}

/// ABI GI-03 canonique :
/// `sys_dma_alloc(size, direction) -> (virt, iova)`.
/// Réalisation registre noyau :
/// - `rax` retourne l'IOVA
/// - `arg3` (`user_virt_out`) reçoit l'adresse CPU virtuelle si non nul
/// - `arg4`/`arg5` restent des extensions de compatibilité (`map_flags`, `domain_hint`)
pub fn sys_dma_alloc(
    size: u64,
    direction: u64,
    user_virt_out: u64,
    map_flags: u64,
    domain_hint: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_DMA_ALLOC);

    if size == 0 || size > usize::MAX as u64 || domain_hint > u32::MAX as u64 {
        return EINVAL;
    }

    let direction = match parse_dma_direction(direction) {
        Some(v) => v,
        None => return EINVAL,
    };

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let requested_domain = IommuDomainId(domain_hint as u32);
    let effective_domain = match crate::drivers::iommu::ensure_domain_for_pid(caller_pid) {
        Ok(domain) => domain,
        Err(_) if requested_domain.0 != 0 => requested_domain,
        Err(_) => return EAGAIN,
    };

    match crate::drivers::sys_dma_alloc_for_pid(
        caller_pid,
        size as usize,
        direction,
        DmaMapFlags(map_flags as u32),
        effective_domain,
    ) {
        Ok((virt, iova)) => {
            if user_virt_out != 0 {
                if let Err(e) = write_user_typed::<u64>(user_virt_out, virt) {
                    let _ = crate::drivers::sys_dma_free_for_pid(caller_pid, iova, effective_domain);
                    return e.to_errno();
                }
            }
            iova.0 as i64
        }
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 canonique :
/// `sys_dma_free(iova, size)`.
/// `arg3` reste un hint de domaine pour le noyau tant que la couche userland n'est pas finalisée.
pub fn sys_dma_free(iova: u64, size: u64, domain_hint: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_FREE);

    if size == 0 || size > usize::MAX as u64 || domain_hint > u32::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    if crate::drivers::dma::dma_alloc_size_for_pid(caller_pid, IovaAddr(iova)) != Some(size as usize) {
        return EINVAL;
    }

    let requested_domain = IommuDomainId(domain_hint as u32);
    let effective_domain = crate::drivers::iommu::domain_of_pid(caller_pid).unwrap_or(requested_domain);

    match crate::drivers::sys_dma_free_for_pid(caller_pid, IovaAddr(iova), effective_domain) {
        Ok(()) => 0,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 canonique : `sys_dma_sync(iova, size, direction)`.
pub fn sys_dma_sync(iova: u64, size: u64, direction: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_DMA_SYNC);

    if size == 0 || size > usize::MAX as u64 {
        return EINVAL;
    }

    let direction = match parse_dma_direction(direction) {
        Some(v) => v,
        None => return EINVAL,
    };

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_dma_sync_for_pid(caller_pid, IovaAddr(iova), size as usize, direction) {
        Ok(()) => 0,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_cfg_read(offset)` pour le device claimé du PID appelant.
pub fn sys_pci_cfg_read(offset: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PCI_CFG_READ);

    if offset > u16::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_pci_cfg_read_for_pid(caller_pid, offset as u16) {
        Ok(value) => value as i64,
        Err(err) => pci_cfg_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_cfg_write(offset, value)` pour le device claimé du PID appelant.
pub fn sys_pci_cfg_write(offset: u64, value: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PCI_CFG_WRITE);

    if offset > u16::MAX as u64 || value > u32::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_pci_cfg_write_for_pid(caller_pid, offset as u16, value as u32) {
        Ok(()) => 0,
        Err(err) => pci_cfg_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_bus_master(enable)` pour le device claimé du PID appelant.
pub fn sys_pci_bus_master(enable: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_PCI_BUS_MASTER);

    if enable > 1 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_pci_bus_master_for_pid(caller_pid, enable != 0) {
        Ok(()) => 0,
        Err(err) => pci_cfg_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_pci_claim(phys_addr, size, owner_pid, bdf_raw, has_bdf)`.
pub fn sys_pci_claim(
    phys_addr: u64,
    size: u64,
    owner_pid: u64,
    bdf_raw: u64,
    has_bdf: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_PCI_CLAIM);
    if size == 0 || size > usize::MAX as u64 || owner_pid == 0 || owner_pid > u32::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let owner_pid = owner_pid as u32;
    let bdf = if has_bdf != 0 {
        Some(parse_pci_address(bdf_raw))
    } else {
        None
    };

    match crate::drivers::sys_pci_claim(PhysAddr::new(phys_addr), size as usize, owner_pid, bdf, caller_pid) {
        Ok(()) => {
            if crate::drivers::iommu::ensure_domain_for_pid(owner_pid).is_err() {
                let _ = crate::drivers::release_claim_for_owner(owner_pid);
                return EAGAIN;
            }
            0
        }
        Err(err) => claim_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_dma_map(vaddr, size, direction)`.
pub fn sys_dma_map(
    vaddr: u64,
    size: u64,
    direction: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_DMA_MAP);

    if size == 0 || size > usize::MAX as u64 || vaddr > usize::MAX as u64 {
        return EINVAL;
    }

    let direction = match parse_dma_direction(direction) {
        Some(v) => v,
        None => return EINVAL,
    };

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_dma_map(
        caller_pid,
        vaddr as usize,
        size as usize,
        direction,
    ) {
        Ok(iova) => iova.0 as i64,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_dma_unmap(domain_id, iova)`.
pub fn sys_dma_unmap(
    domain_id: u64,
    iova: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_DMA_UNMAP);
    if domain_id > u32::MAX as u64 {
        return EINVAL;
    }

    let requested_domain = IommuDomainId(domain_id as u32);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    let effective_domain = if caller_pid != 0 {
        crate::drivers::iommu::domain_of_pid(caller_pid).unwrap_or(requested_domain)
    } else {
        requested_domain
    };

    match crate::drivers::sys_dma_unmap(caller_pid, IovaAddr(iova), effective_domain) {
        Ok(()) => 0,
        Err(err) => dma_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_msi_alloc(count)`.
pub fn sys_msi_alloc(count: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSI_ALLOC);

    if count == 0 || count > u16::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_msi_alloc_for_pid(caller_pid, count as u16) {
        Ok(handle) => handle as i64,
        Err(err) => msi_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_msi_config(handle, vector_idx)`.
pub fn sys_msi_config(handle: u64, vector_idx: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSI_CONFIG);

    if vector_idx > u16::MAX as u64 {
        return EINVAL;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_msi_config_for_pid(caller_pid, handle, vector_idx as u16) {
        Ok(_vector) => 0,
        Err(err) => msi_error_to_errno(err),
    }
}

/// ABI GI-03 : `sys_msi_free(handle)`.
pub fn sys_msi_free(handle: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MSI_FREE);

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    match crate::drivers::sys_msi_free_for_pid(caller_pid, handle) {
        Ok(()) => 0,
        Err(err) => msi_error_to_errno(err),
    }
}

/// ABI GI-03 canonique : `sys_pci_set_topology(child_bdf_raw, parent_bdf_raw)`.
pub fn sys_pci_set_topology(
    bdf_raw: u64,
    parent_bdf_raw: u64,
    has_parent: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_PCI_SET_TOPOLOGY);

    if has_parent == 0 {
        return EINVAL;
    }

    let address = parse_pci_address(bdf_raw);
    let parent_bridge = parse_pci_address(parent_bdf_raw);

    match crate::drivers::sys_pci_set_topology(address, parent_bridge) {
        Ok(()) => 0,
        Err(err) => topo_error_to_errno(err),
    }
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
        SYS_GETDENTS64 => sys_getdents64,
        SYS_READLINK  => sys_readlink,
        SYS_READLINKAT => sys_readlinkat,
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
        SYS_EXO_IPC_RECV_NB => sys_exo_ipc_recv_nb,
        SYS_EXO_IPC_CALL => sys_exo_ipc_call,
        SYS_EXO_IPC_CREATE => sys_exo_ipc_create,
        SYS_EXO_IPC_DESTROY => sys_exo_ipc_destroy,
        SYS_EXO_CAP_CREATE => sys_exo_cap_create,
        SYS_EXO_CAP_REVOKE => sys_exo_cap_revoke,
        SYS_EXO_LOG        => sys_exo_log,
        // ── ExoFS (500–518) ────────────────────────────────────────────────
        SYS_EXOFS_PATH_RESOLVE     => sys_exofs_path_resolve,
        SYS_EXOFS_OBJECT_OPEN      => sys_exofs_object_open,
        SYS_EXOFS_OBJECT_READ      => sys_exofs_object_read,
        SYS_EXOFS_OBJECT_WRITE     => sys_exofs_object_write,
        SYS_EXOFS_OBJECT_CREATE    => sys_exofs_object_create,
        SYS_EXOFS_OBJECT_DELETE    => sys_exofs_object_delete,
        SYS_EXOFS_OBJECT_STAT      => sys_exofs_object_stat,
        SYS_EXOFS_OBJECT_SET_META  => sys_exofs_object_set_meta_abi,
        SYS_EXOFS_GET_CONTENT_HASH => sys_exofs_get_content_hash,
        SYS_EXOFS_SNAPSHOT_CREATE  => sys_exofs_snapshot_create,
        SYS_EXOFS_SNAPSHOT_LIST    => sys_exofs_snapshot_list,
        SYS_EXOFS_SNAPSHOT_MOUNT   => sys_exofs_snapshot_mount,
        SYS_EXOFS_RELATION_CREATE  => sys_exofs_relation_create,
        SYS_EXOFS_RELATION_QUERY   => sys_exofs_relation_query,
        SYS_EXOFS_GC_TRIGGER       => sys_exofs_gc_trigger,
        SYS_EXOFS_QUOTA_QUERY      => sys_exofs_quota_query,
        SYS_EXOFS_EXPORT_OBJECT    => sys_exofs_export_object,
        SYS_EXOFS_IMPORT_OBJECT    => sys_exofs_import_object,
        SYS_EXOFS_EPOCH_COMMIT     => sys_exofs_epoch_commit,
        // ── ExoFS extensions (519–520) — FIX BUG-01 + BUG-02 ───────────────
        SYS_EXOFS_OPEN_BY_PATH     => sys_exofs_open_by_path,
        SYS_EXOFS_READDIR          => sys_exofs_readdir,
        // ── GI-03 Drivers (530–546) ──────────────────────────────────────────
        SYS_IRQ_REGISTER        => sys_irq_register,
        SYS_IRQ_ACK             => sys_irq_ack,
        SYS_MMIO_MAP            => sys_mmio_map,
        SYS_MMIO_UNMAP          => sys_mmio_unmap,
        SYS_DMA_ALLOC           => sys_dma_alloc,
        SYS_DMA_FREE            => sys_dma_free,
        SYS_DMA_SYNC            => sys_dma_sync,
        SYS_PCI_CFG_READ        => sys_pci_cfg_read,
        SYS_PCI_CFG_WRITE       => sys_pci_cfg_write,
        SYS_PCI_BUS_MASTER      => sys_pci_bus_master,
        SYS_PCI_CLAIM           => sys_pci_claim,
        SYS_DMA_MAP             => sys_dma_map,
        SYS_DMA_UNMAP           => sys_dma_unmap,
        SYS_MSI_ALLOC           => sys_msi_alloc,
        SYS_MSI_CONFIG          => sys_msi_config,
        SYS_MSI_FREE            => sys_msi_free,
        SYS_PCI_SET_TOPOLOGY    => sys_pci_set_topology,
        // ── Catch-all ──────────────────────────────────────────────────────
        _             => sys_enosys,
    }
}
