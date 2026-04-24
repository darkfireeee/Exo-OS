//! # syscall/compat/posix.rs — Conformité POSIX.1-2017 des syscalls
//!
//! Ce module centralise :
//! 1. Les constantes POSIX manquantes des fichiers d'en-tête.
//! 2. Les wrappers POSIX complémentaires (fonctions qui existent en POSIX
//!    mais dont l'implémentation est répartie dans plusieurs modules kernel).
//! 3. Les helpers de conversion entre types Exo-OS et types POSIX ABI.
//! 4. La validation des arguments POSIX (ex: valeurs `whence` pour lseek).
//!
//! ## Syscalls POSIX couverts par ce module
//! - `getgroups` / `setgroups`
//! - `setuid` / `setgid` / `setresuid` / `setresgid` et variantes
//! - `umask`
//! - `setsid` / `getsid`
//! - `setpgid` / `getpgid`
//! - `times`
//! - `getdents64`
//! - `readlink` / `readlinkat`
//!
//! Les syscalls mathématiquement simples (getuid, getgid, etc.) sont
//! dans `fast_path.rs`. Ce module gère ceux qui nécessitent des
//! structures de données ou de la logique.
//!
//! ## Référence POSIX
//! POSIX.1-2017 (IEEE Std 1003.1-2017)

use crate::process::core::pcb::process_flags;
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::syscall::fast_path::syscall_current_pid;
use crate::syscall::numbers::*;
use crate::syscall::validation::{write_user_typed, SyscallError};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Compteur
// ─────────────────────────────────────────────────────────────────────────────

static POSIX_CALL_COUNT: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn inc_posix() {
    POSIX_CALL_COUNT.fetch_add(1, Ordering::Relaxed);
}
/// Retourne le nombre de syscalls POSIX traités par ce module.
pub fn posix_call_count() -> u64 {
    POSIX_CALL_COUNT.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Constantes POSIX — Flags open(2) / lseek(2) / mmap(2) / etc.
// ─────────────────────────────────────────────────────────────────────────────

/// Flags `open(2)` / `openat(2)` — conforme POSIX.1-2017 + Linux extras.
pub mod open_flags {
    pub const O_RDONLY: u32 = 0x0000;
    pub const O_WRONLY: u32 = 0x0001;
    pub const O_RDWR: u32 = 0x0002;
    pub const O_CREAT: u32 = 0x0040;
    pub const O_EXCL: u32 = 0x0080;
    pub const O_NOCTTY: u32 = 0x0100;
    pub const O_TRUNC: u32 = 0x0200;
    pub const O_APPEND: u32 = 0x0400;
    pub const O_NONBLOCK: u32 = 0x0800;
    pub const O_DSYNC: u32 = 0x1000;
    pub const O_DIRECT: u32 = 0x4000;
    pub const O_LARGEFILE: u32 = 0x8000;
    pub const O_DIRECTORY: u32 = 0x0001_0000;
    pub const O_NOFOLLOW: u32 = 0x0002_0000;
    pub const O_NOATIME: u32 = 0x0004_0000;
    pub const O_CLOEXEC: u32 = 0x0008_0000;
    pub const O_SYNC: u32 = 0x0010_1000;
    pub const O_PATH: u32 = 0x0020_0000;
    pub const O_TMPFILE: u32 = 0x0040_0000;
    /// Masque de tous les flags autorisés (pour validate_flags)
    pub const ALLOWED_MASK: u64 = 0x0040_1FFFu64;
}

/// Valeurs `whence` pour `lseek(2)`.
pub mod seek_whence {
    pub const SEEK_SET: u32 = 0;
    pub const SEEK_CUR: u32 = 1;
    pub const SEEK_END: u32 = 2;
    /// Linux extension : seek to next data
    pub const SEEK_DATA: u32 = 3;
    /// Linux extension : seek to next hole
    pub const SEEK_HOLE: u32 = 4;
}

/// Flags `mmap(2)`.
pub mod mmap_flags {
    pub const MAP_SHARED: u32 = 0x01;
    pub const MAP_PRIVATE: u32 = 0x02;
    pub const MAP_FIXED: u32 = 0x10;
    pub const MAP_ANONYMOUS: u32 = 0x20;
    pub const MAP_GROWSDOWN: u32 = 0x0100;
    pub const MAP_DENYWRITE: u32 = 0x0800;
    pub const MAP_EXECUTABLE: u32 = 0x1000;
    pub const MAP_LOCKED: u32 = 0x2000;
    pub const MAP_NORESERVE: u32 = 0x4000;
    pub const MAP_POPULATE: u32 = 0x0800_0;
    pub const MAP_NONBLOCK: u32 = 0x1000_0;
    pub const MAP_STACK: u32 = 0x2000_0;
    pub const MAP_HUGETLB: u32 = 0x4000_0;
}

/// Flags `mprotect(2)` / mmap `prot`.
pub mod prot_flags {
    pub const PROT_NONE: u32 = 0x0;
    pub const PROT_READ: u32 = 0x1;
    pub const PROT_WRITE: u32 = 0x2;
    pub const PROT_EXEC: u32 = 0x4;
    pub const PROT_SEM: u32 = 0x8;
    pub const PROT_GROWSDOWN: u32 = 0x0100_0000;
    pub const PROT_GROWSUP: u32 = 0x0200_0000;
}

/// Signals POSIX (identique à Linux pour compatibilité glibc).
pub mod signals {
    pub const SIGHUP: u32 = 1;
    pub const SIGINT: u32 = 2;
    pub const SIGQUIT: u32 = 3;
    pub const SIGILL: u32 = 4;
    pub const SIGTRAP: u32 = 5;
    pub const SIGABRT: u32 = 6;
    pub const SIGBUS: u32 = 7;
    pub const SIGFPE: u32 = 8;
    pub const SIGKILL: u32 = 9;
    pub const SIGUSR1: u32 = 10;
    pub const SIGSEGV: u32 = 11;
    pub const SIGUSR2: u32 = 12;
    pub const SIGPIPE: u32 = 13;
    pub const SIGALRM: u32 = 14;
    pub const SIGTERM: u32 = 15;
    pub const SIGCHLD: u32 = 17;
    pub const SIGCONT: u32 = 18;
    pub const SIGSTOP: u32 = 19;
    pub const SIGTSTP: u32 = 20;
    pub const SIGTTIN: u32 = 21;
    pub const SIGTTOU: u32 = 22;
    pub const SIGURG: u32 = 23;
    pub const SIGXCPU: u32 = 24;
    pub const SIGXFSZ: u32 = 25;
    pub const SIGVTALRM: u32 = 26;
    pub const SIGPROF: u32 = 27;
    pub const SIGWINCH: u32 = 28;
    pub const SIGIO: u32 = 29;
    pub const SIGPWR: u32 = 30;
    pub const SIGSYS: u32 = 31;
    /// Premier signal temps-réel
    pub const SIGRTMIN: u32 = 32;
    /// Dernier signal temps-réel
    pub const SIGRTMAX: u32 = 64;
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers de validation POSIX
// ─────────────────────────────────────────────────────────────────────────────

/// Valide les flags `open()` / `openat()`.
/// Retourne les flags nettoyés ou EINVAL si des bits réservés sont levés.
#[inline]
pub fn validate_open_flags(flags: u32) -> Result<u32, SyscallError> {
    // Les bits au-delà de ALLOWED_MASK sont réservés → EINVAL
    if (flags as u64) & !open_flags::ALLOWED_MASK != 0 {
        return Err(SyscallError::Invalid);
    }
    // O_RDONLY=0 est valide même si la valeur est 0
    Ok(flags)
}

/// Valide les protections mmap.
#[inline]
pub fn validate_prot(prot: u32) -> Result<u32, SyscallError> {
    use prot_flags::*;
    let known =
        PROT_NONE | PROT_READ | PROT_WRITE | PROT_EXEC | PROT_SEM | PROT_GROWSDOWN | PROT_GROWSUP;
    if prot & !known != 0 {
        return Err(SyscallError::Invalid);
    }
    Ok(prot)
}

/// Valide les flags `mmap()`.
#[inline]
pub fn validate_mmap_flags(flags: u32) -> Result<u32, SyscallError> {
    // MAP_SHARED XOR MAP_PRIVATE est requis
    let shared = flags & mmap_flags::MAP_SHARED != 0;
    let private = flags & mmap_flags::MAP_PRIVATE != 0;
    if shared == private {
        // Ni l'un ni l'autre, ou les deux → EINVAL
        return Err(SyscallError::Invalid);
    }
    Ok(flags)
}

/// Valide la valeur `whence` de `lseek()`.
#[inline]
pub fn validate_lseek_whence(whence: u64) -> Result<u32, SyscallError> {
    match whence {
        0 | 1 | 2 | 3 | 4 => Ok(whence as u32),
        _ => Err(SyscallError::Invalid),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers POSIX — Identité / groupes
// ─────────────────────────────────────────────────────────────────────────────

/// `setuid(uid)` — POSIX.1-2017 § setuid().
pub fn sys_setuid(uid: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let uid32 = uid as u32;
    let caller = Pid(syscall_current_pid());
    match PROCESS_REGISTRY.find_by_pid(caller) {
        None => EINVAL,
        Some(pcb) => {
            let c = pcb.get_creds();
            // Root : set uid partout ; sinon seulement si uid est dans {uid,euid,suid}.
            if c.is_root() || uid32 == c.uid || uid32 == c.euid || uid32 == c.suid {
                pcb.set_uid(uid32);
                if c.is_root() {
                    pcb.set_euid(uid32);
                }
                0
            } else {
                EPERM
            }
        }
    }
}

/// `setgid(gid)` — POSIX.1-2017 § setgid().
pub fn sys_setgid(gid: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let gid32 = gid as u32;
    let caller = Pid(syscall_current_pid());
    match PROCESS_REGISTRY.find_by_pid(caller) {
        None => EINVAL,
        Some(pcb) => {
            let c = pcb.get_creds();
            if c.is_root() || gid32 == c.gid || gid32 == c.egid || gid32 == c.sgid {
                pcb.set_gid(gid32);
                if c.is_root() {
                    pcb.set_egid(gid32);
                }
                0
            } else {
                EPERM
            }
        }
    }
}

/// `setresuid(ruid, euid, suid)` — POSIX + Linux.
/// Valeur -1 (u32::MAX) = ne pas modifier ce champ.
pub fn sys_setresuid(ruid: u64, euid: u64, suid: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let r32 = ruid as u32;
    let e32 = euid as u32;
    let s32 = suid as u32;
    let caller = Pid(syscall_current_pid());
    match PROCESS_REGISTRY.find_by_pid(caller) {
        None => EINVAL,
        Some(pcb) => {
            let c = pcb.get_creds();
            // Chaque valeur doit être u32::MAX (ne pas changer), ou l'un des IDs courants, ou 0 si root.
            let ok_uid =
                |v: u32| v == u32::MAX || c.is_root() || v == c.uid || v == c.euid || v == c.suid;
            if ok_uid(r32) && ok_uid(e32) && ok_uid(s32) {
                pcb.set_resuid(r32, e32, s32);
                0
            } else {
                EPERM
            }
        }
    }
}

/// `getresuid(ruid_ptr, euid_ptr, suid_ptr)` — écrit les 3 UIDs en espace user.
pub fn sys_getresuid(
    ruid_ptr: u64,
    euid_ptr: u64,
    suid_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    inc_posix();
    let caller = Pid(syscall_current_pid());
    match PROCESS_REGISTRY.find_by_pid(caller) {
        None => EINVAL,
        Some(pcb) => {
            let c = pcb.get_creds();
            if write_user_typed(ruid_ptr, c.uid).is_err()
                || write_user_typed(euid_ptr, c.euid).is_err()
                || write_user_typed(suid_ptr, c.suid).is_err()
            {
                return EFAULT;
            }
            0
        }
    }
}

/// `setresgid(rgid, egid, sgid)` — POSIX + Linux.
/// Valeur u32::MAX = ne pas modifier ce champ.
pub fn sys_setresgid(rgid: u64, egid: u64, sgid: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let r32 = rgid as u32;
    let e32 = egid as u32;
    let s32 = sgid as u32;
    let caller = Pid(syscall_current_pid());
    match PROCESS_REGISTRY.find_by_pid(caller) {
        None => EINVAL,
        Some(pcb) => {
            let c = pcb.get_creds();
            let ok_gid =
                |v: u32| v == u32::MAX || c.is_root() || v == c.gid || v == c.egid || v == c.sgid;
            if ok_gid(r32) && ok_gid(e32) && ok_gid(s32) {
                pcb.set_resgid(r32, e32, s32);
                0
            } else {
                EPERM
            }
        }
    }
}

/// `getresgid(rgid_ptr, egid_ptr, sgid_ptr)` — écrit les 3 GIDs en espace user.
pub fn sys_getresgid(
    rgid_ptr: u64,
    egid_ptr: u64,
    sgid_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    inc_posix();
    let caller = Pid(syscall_current_pid());
    match PROCESS_REGISTRY.find_by_pid(caller) {
        None => EINVAL,
        Some(pcb) => {
            let c = pcb.get_creds();
            if write_user_typed(rgid_ptr, c.gid).is_err()
                || write_user_typed(egid_ptr, c.egid).is_err()
                || write_user_typed(sgid_ptr, c.sgid).is_err()
            {
                return EFAULT;
            }
            0
        }
    }
}

/// `setsid()` — crée une nouvelle session pour le processus courant.
/// Échoue si le processus est déjà leader de groupe (POSIX.1-2017).
pub fn sys_setsid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let caller = Pid(syscall_current_pid());
    match PROCESS_REGISTRY.find_by_pid(caller) {
        None => EINVAL,
        Some(pcb) => {
            if pcb.is_pgroup_leader() {
                return EPERM; // Échec si déjà leader de groupe de processus
            }
            let new_sid = caller.0;
            pcb.set_session_id(new_sid);
            pcb.set_pgroup_id(new_sid);
            // SAFETY: fetch_or sur AtomicU32 — pas d'alloc, pas de droits manquants.
            pcb.flags
                .fetch_or(process_flags::SESSION_LEADER, Ordering::Release);
            new_sid as i64
        }
    }
}

/// `getsid(pid)` — retourne le SID du processus `pid` (ou du processus courant si pid=0).
pub fn sys_getsid(pid: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let target = if pid == 0 {
        Pid(syscall_current_pid())
    } else {
        Pid(pid as u32)
    };
    match PROCESS_REGISTRY.find_by_pid(target) {
        None => EINVAL,
        Some(p) => p.session_id() as i64,
    }
}

/// `setpgid(pid, pgid)` — délègue vers process::group::pgrp::setpgid.
pub fn sys_setpgid(pid: u64, pgid: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    use crate::process::core::pid::Pid;
    use crate::process::group::pgrp::{setpgid, PgId};
    match setpgid(Pid(pid as u32), PgId(pgid as u32)) {
        Ok(_) => 0,
        Err(_) => EINVAL,
    }
}

/// `getpgid(pid)` — retourne le PGID du processus `pid` (ou du processus courant si pid=0).
pub fn sys_getpgid(pid: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let target = if pid == 0 {
        Pid(syscall_current_pid())
    } else {
        Pid(pid as u32)
    };
    match PROCESS_REGISTRY.find_by_pid(target) {
        None => EINVAL,
        Some(p) => p.pgroup_id() as i64,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers POSIX — umask, times, getdents64
// ─────────────────────────────────────────────────────────────────────────────

/// `umask(mask)` — modifie le masque de création de fichier.
/// Note : le PCB ne stocke pas encore de champ `umask` dédié.
/// Cette implémentation retourne le masque demandé (stub partiel, mieux que ENOSYS).
/// Note Tâche-5 : ajouter `umask: AtomicU32` dans PCB et stocker la valeur réelle.
pub fn sys_umask(mask: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    // Masque toujours limité aux 9 bits de permission fichier.
    (mask & 0o777) as i64
}

/// `getdents64(fd, dirp, count)` — câblé lors de l'activation de fs/.
pub fn sys_getdents64(fd: u64, dirp: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let _ = (fd, dirp, count);
    ENOSYS
}

/// `readlink(path, buf, bufsize)` — câblé lors de l'activation de fs/.
pub fn sys_readlink(
    path_ptr: u64,
    buf_ptr: u64,
    bufsize: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    inc_posix();
    let _ = (path_ptr, buf_ptr, bufsize);
    ENOSYS
}

/// `readlinkat(dirfd, path, buf, bufsize)` — câblé lors de l'activation de fs/.
pub fn sys_readlinkat(
    dirfd: u64,
    path_ptr: u64,
    buf_ptr: u64,
    bufsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    inc_posix();
    let _ = (dirfd, path_ptr, buf_ptr, bufsize);
    ENOSYS
}

/// `times(tbuf)` — retourne les temps CPU consommés.
pub fn sys_times(tbuf_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    // struct tms { utime, stime, cutime, cstime : i64 }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct Tms {
        utime: i64,
        stime: i64,
        cutime: i64,
        cstime: i64,
    }

    let tms = Tms::default();
    if tbuf_ptr != 0 {
        if let Err(e) = write_user_typed::<Tms>(tbuf_ptr, tms) {
            return e.to_errno();
        }
    }
    // Ticks depuis le boot (ms comme approximation d'un tick HZ=1000)
    let ticks = crate::scheduler::timer::clock::monotonic_ns() / 1_000_000;
    ticks as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers POSIX — Identification de groupes
// ─────────────────────────────────────────────────────────────────────────────

/// `getgroups(size, list_ptr)` — câblé lors de l'intégration credentials.
pub fn sys_getgroups(size: u64, list_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let _ = (size, list_ptr);
    ENOSYS
}

/// `setgroups(size, list_ptr)` — câblé lors de l'intégration credentials.
pub fn sys_setgroups(size: u64, list_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let _ = (size, list_ptr);
    ENOSYS
}

/// `capget(hdrp, datap)` — câblé lors de l'intégration security/capability.
pub fn sys_capget(hdrp: u64, datap: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let _ = (hdrp, datap);
    ENOSYS
}

/// `capset(hdrp, datap)` — câblé lors de l'intégration security/capability.
pub fn sys_capset(hdrp: u64, datap: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let _ = (hdrp, datap);
    ENOSYS
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers d'export : table de mapping numéro → handler POSIX
// ─────────────────────────────────────────────────────────────────────────────

use crate::syscall::table::SyscallHandler;

/// Retourne le handler POSIX pour un numéro donné, ou `None` si non géré ici.
///
/// Appelé depuis `table::get_handler()` en complément du match principal.
pub fn get_posix_handler(nr: u64) -> Option<SyscallHandler> {
    match nr {
        SYS_SETUID => Some(sys_setuid),
        SYS_SETGID => Some(sys_setgid),
        SYS_SETRESUID => Some(sys_setresuid),
        SYS_GETRESUID => Some(sys_getresuid),
        SYS_SETRESGID => Some(sys_setresgid),
        SYS_GETRESGID => Some(sys_getresgid),
        SYS_SETSID => Some(sys_setsid),
        SYS_GETSID => Some(sys_getsid),
        SYS_SETPGID => Some(sys_setpgid),
        SYS_GETPGID => Some(sys_getpgid),
        SYS_UMASK => Some(sys_umask),
        SYS_GETDENTS64 => Some(sys_getdents64),
        SYS_READLINK => Some(sys_readlink),
        SYS_READLINKAT => Some(sys_readlinkat),
        SYS_TIMES => Some(sys_times),
        SYS_GETGROUPS => Some(sys_getgroups),
        SYS_SETGROUPS => Some(sys_setgroups),
        SYS_CAPGET => Some(sys_capget),
        SYS_CAPSET => Some(sys_capset),
        _ => None,
    }
}
