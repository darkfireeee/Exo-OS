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

#![allow(dead_code)]
#![allow(unused_variables)]

use core::sync::atomic::{AtomicU64, Ordering};
use crate::syscall::numbers::*;
use crate::syscall::validation::{
    SyscallError, write_user_typed, read_user_typed, read_user_path,
    validate_fd, validate_pid,
};

// ─────────────────────────────────────────────────────────────────────────────
// Compteur
// ─────────────────────────────────────────────────────────────────────────────

static POSIX_CALL_COUNT: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn inc_posix() { POSIX_CALL_COUNT.fetch_add(1, Ordering::Relaxed); }
/// Retourne le nombre de syscalls POSIX traités par ce module.
pub fn posix_call_count() -> u64 { POSIX_CALL_COUNT.load(Ordering::Relaxed) }

// ─────────────────────────────────────────────────────────────────────────────
// Constantes POSIX — Flags open(2) / lseek(2) / mmap(2) / etc.
// ─────────────────────────────────────────────────────────────────────────────

/// Flags `open(2)` / `openat(2)` — conforme POSIX.1-2017 + Linux extras.
pub mod open_flags {
    pub const O_RDONLY:    u32 = 0x0000;
    pub const O_WRONLY:    u32 = 0x0001;
    pub const O_RDWR:      u32 = 0x0002;
    pub const O_CREAT:     u32 = 0x0040;
    pub const O_EXCL:      u32 = 0x0080;
    pub const O_NOCTTY:    u32 = 0x0100;
    pub const O_TRUNC:     u32 = 0x0200;
    pub const O_APPEND:    u32 = 0x0400;
    pub const O_NONBLOCK:  u32 = 0x0800;
    pub const O_DSYNC:     u32 = 0x1000;
    pub const O_DIRECT:    u32 = 0x4000;
    pub const O_LARGEFILE: u32 = 0x8000;
    pub const O_DIRECTORY: u32 = 0x0001_0000;
    pub const O_NOFOLLOW:  u32 = 0x0002_0000;
    pub const O_NOATIME:   u32 = 0x0004_0000;
    pub const O_CLOEXEC:   u32 = 0x0008_0000;
    pub const O_SYNC:      u32 = 0x0010_1000;
    pub const O_PATH:      u32 = 0x0020_0000;
    pub const O_TMPFILE:   u32 = 0x0040_0000;
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
    pub const MAP_SHARED:     u32 = 0x01;
    pub const MAP_PRIVATE:    u32 = 0x02;
    pub const MAP_FIXED:      u32 = 0x10;
    pub const MAP_ANONYMOUS:  u32 = 0x20;
    pub const MAP_GROWSDOWN:  u32 = 0x0100;
    pub const MAP_DENYWRITE:  u32 = 0x0800;
    pub const MAP_EXECUTABLE: u32 = 0x1000;
    pub const MAP_LOCKED:     u32 = 0x2000;
    pub const MAP_NORESERVE:  u32 = 0x4000;
    pub const MAP_POPULATE:   u32 = 0x0800_0;
    pub const MAP_NONBLOCK:   u32 = 0x1000_0;
    pub const MAP_STACK:      u32 = 0x2000_0;
    pub const MAP_HUGETLB:    u32 = 0x4000_0;
}

/// Flags `mprotect(2)` / mmap `prot`.
pub mod prot_flags {
    pub const PROT_NONE:  u32 = 0x0;
    pub const PROT_READ:  u32 = 0x1;
    pub const PROT_WRITE: u32 = 0x2;
    pub const PROT_EXEC:  u32 = 0x4;
    pub const PROT_SEM:   u32 = 0x8;
    pub const PROT_GROWSDOWN: u32 = 0x0100_0000;
    pub const PROT_GROWSUP:   u32 = 0x0200_0000;
}

/// Signals POSIX (identique à Linux pour compatibilité glibc).
pub mod signals {
    pub const SIGHUP:    u32 =  1;
    pub const SIGINT:    u32 =  2;
    pub const SIGQUIT:   u32 =  3;
    pub const SIGILL:    u32 =  4;
    pub const SIGTRAP:   u32 =  5;
    pub const SIGABRT:   u32 =  6;
    pub const SIGBUS:    u32 =  7;
    pub const SIGFPE:    u32 =  8;
    pub const SIGKILL:   u32 =  9;
    pub const SIGUSR1:   u32 = 10;
    pub const SIGSEGV:   u32 = 11;
    pub const SIGUSR2:   u32 = 12;
    pub const SIGPIPE:   u32 = 13;
    pub const SIGALRM:   u32 = 14;
    pub const SIGTERM:   u32 = 15;
    pub const SIGCHLD:   u32 = 17;
    pub const SIGCONT:   u32 = 18;
    pub const SIGSTOP:   u32 = 19;
    pub const SIGTSTP:   u32 = 20;
    pub const SIGTTIN:   u32 = 21;
    pub const SIGTTOU:   u32 = 22;
    pub const SIGURG:    u32 = 23;
    pub const SIGXCPU:   u32 = 24;
    pub const SIGXFSZ:   u32 = 25;
    pub const SIGVTALRM: u32 = 26;
    pub const SIGPROF:   u32 = 27;
    pub const SIGWINCH:  u32 = 28;
    pub const SIGIO:     u32 = 29;
    pub const SIGPWR:    u32 = 30;
    pub const SIGSYS:    u32 = 31;
    /// Premier signal temps-réel
    pub const SIGRTMIN:  u32 = 32;
    /// Dernier signal temps-réel
    pub const SIGRTMAX:  u32 = 64;
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
    let known = PROT_NONE | PROT_READ | PROT_WRITE | PROT_EXEC | PROT_SEM
                | PROT_GROWSDOWN | PROT_GROWSUP;
    if prot & !known != 0 {
        return Err(SyscallError::Invalid);
    }
    Ok(prot)
}

/// Valide les flags `mmap()`.
#[inline]
pub fn validate_mmap_flags(flags: u32) -> Result<u32, SyscallError> {
    // MAP_SHARED XOR MAP_PRIVATE est requis
    let shared  = flags & mmap_flags::MAP_SHARED  != 0;
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

/// `setuid(uid)` — change l'UID réel et effectif.
pub fn sys_setuid(uid: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if uid > u32::MAX as u64 { return EINVAL; }
    match crate::process::core::registry::PROCESS_REGISTRY.setuid(uid as u32) {
        Ok(_)  => 0,
        Err(e) => e.to_errno(),
    }
}

/// `setgid(gid)`.
pub fn sys_setgid(gid: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if gid > u32::MAX as u64 { return EINVAL; }
    match crate::process::core::registry::PROCESS_REGISTRY.setgid(gid as u32) {
        Ok(_)  => 0,
        Err(e) => e.to_errno(),
    }
}

/// `setresuid(ruid, euid, suid)` — change les UIDs réel, effectif et sauvegardé.
pub fn sys_setresuid(ruid: u64, euid: u64, suid: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    match crate::process::core::registry::PROCESS_REGISTRY.setresuid(
        ruid as i64, euid as i64, suid as i64
    ) {
        Ok(_)  => 0,
        Err(e) => e.to_errno(),
    }
}

/// `getresuid(ruid_ptr, euid_ptr, suid_ptr)`.
pub fn sys_getresuid(ruid_ptr: u64, euid_ptr: u64, suid_ptr: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if ruid_ptr == 0 || euid_ptr == 0 || suid_ptr == 0 { return EFAULT; }
    if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.current_pcb() {
        let ruid = pcb.uid.load(Ordering::Relaxed);
        let euid = pcb.euid.load(Ordering::Relaxed);
        let suid = pcb.suid.load(Ordering::Relaxed);
        let _ = write_user_typed::<u32>(ruid_ptr, ruid);
        let _ = write_user_typed::<u32>(euid_ptr, euid);
        let _ = write_user_typed::<u32>(suid_ptr, suid);
        0
    } else {
        ESRCH
    }
}

/// `setresgid(rgid, egid, sgid)`.
pub fn sys_setresgid(rgid: u64, egid: u64, sgid: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    match crate::process::core::registry::PROCESS_REGISTRY.setresgid(
        rgid as i64, egid as i64, sgid as i64
    ) {
        Ok(_)  => 0,
        Err(e) => e.to_errno(),
    }
}

/// `getresgid(rgid_ptr, egid_ptr, sgid_ptr)`.
pub fn sys_getresgid(rgid_ptr: u64, egid_ptr: u64, sgid_ptr: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if rgid_ptr == 0 || egid_ptr == 0 || sgid_ptr == 0 { return EFAULT; }
    if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.current_pcb() {
        let _ = write_user_typed::<u32>(rgid_ptr, pcb.gid.load(Ordering::Relaxed));
        let _ = write_user_typed::<u32>(egid_ptr, pcb.egid.load(Ordering::Relaxed));
        let _ = write_user_typed::<u32>(sgid_ptr, pcb.sgid.load(Ordering::Relaxed));
        0
    } else {
        ESRCH
    }
}

/// Errno non défini dans numbers.rs : ESRCH (no such process)
const ESRCH: i64 = -3;

/// `setsid()` — crée une nouvelle session.
pub fn sys_setsid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    match crate::process::group::session::setsid() {
        Ok(sid) => sid.0 as i64,
        Err(e)  => e.to_errno(),
    }
}

/// `getsid(pid)`.
pub fn sys_getsid(pid: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    match crate::process::group::session::getsid(pid as u32) {
        Ok(sid) => sid.0 as i64,
        Err(e)  => e.to_errno(),
    }
}

/// `setpgid(pid, pgid)`.
pub fn sys_setpgid(pid: u64, pgid: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    match crate::process::group::pgroup::setpgid(pid as u32, pgid as u32) {
        Ok(_)  => 0,
        Err(e) => e.to_errno(),
    }
}

/// `getpgid(pid)`.
pub fn sys_getpgid(pid: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    match crate::process::group::pgroup::getpgid(pid as u32) {
        Ok(pgid) => pgid.0 as i64,
        Err(e)   => e.to_errno(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers POSIX — umask, times, getdents64
// ─────────────────────────────────────────────────────────────────────────────

/// `umask(mask)` — change le masque de création de fichiers.
pub fn sys_umask(mask: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let old = crate::process::core::registry::PROCESS_REGISTRY.set_umask((mask & 0o777) as u32);
    old as i64
}

/// `getdents64(fd, dirp, count)` — lit des entrées de répertoire en format 64-bit.
pub fn sys_getdents64(fd: u64, dirp: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let fd = match validate_fd(fd) { Ok(f) => f, Err(e) => return e.to_errno() };
    let len = (count as usize).min(crate::syscall::validation::IO_BUF_MAX);
    if dirp == 0 { return EFAULT; }
    match crate::fs::core::vfs::getdents64(fd, dirp, len) {
        Ok(n)  => n as i64,
        Err(e) => e.to_errno() as i64,
    }
}

/// `readlink(path, buf, bufsize)`.
pub fn sys_readlink(path_ptr: u64, buf_ptr: u64, bufsize: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    let len = (bufsize as usize).min(crate::syscall::validation::PATH_MAX);
    if buf_ptr == 0 { return EFAULT; }
    match crate::fs::core::vfs::readlink(path.as_bytes(), buf_ptr, len) {
        Ok(n)  => n as i64,
        Err(e) => e.to_errno() as i64,
    }
}

/// `readlinkat(dirfd, path, buf, bufsize)`.
pub fn sys_readlinkat(dirfd: u64, path_ptr: u64, buf_ptr: u64, bufsize: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let path = match read_user_path(path_ptr) {
        Ok(p) => p, Err(e) => return e.to_errno()
    };
    let len = (bufsize as usize).min(crate::syscall::validation::PATH_MAX);
    if buf_ptr == 0 { return EFAULT; }
    match crate::fs::core::vfs::readlinkat(dirfd as i32, path.as_bytes(), buf_ptr, len) {
        Ok(n)  => n as i64,
        Err(e) => e.to_errno() as i64,
    }
}

/// `times(tbuf)` — retourne les temps CPU consommés.
pub fn sys_times(tbuf_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    // struct tms { tms_utime, tms_stime, tms_cutime, tms_cstime : clock_t (i64) }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct Tms { utime: i64, stime: i64, cutime: i64, cstime: i64 }

    // Lire les temps du thread courant depuis le TCB
    let tms = Tms::default(); // Rempli depuis TCB stats quand process/ sera complet
    if tbuf_ptr != 0 {
        if let Err(e) = write_user_typed::<Tms>(tbuf_ptr, tms) {
            return e.to_errno();
        }
    }
    // Retourne le nombre de ticks d'horloge depuis le boot
    let ticks = crate::scheduler::timer::clock::monotonic_ns() / 1_000_000; // ms comme tick
    ticks as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers POSIX — Identification de groupes
// ─────────────────────────────────────────────────────────────────────────────

/// `getgroups(size, list_ptr)` — retourne les groupes supplémentaires.
pub fn sys_getgroups(size: u64, list_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if size == 0 {
        // size=0 → retourne juste le nombre de groupes
        return match crate::process::core::registry::PROCESS_REGISTRY.get_groups_count() {
            Ok(n) => n as i64,
            Err(e) => e.to_errno(),
        };
    }
    if list_ptr == 0 { return EFAULT; }
    match crate::process::core::registry::PROCESS_REGISTRY.get_groups(list_ptr, size as usize) {
        Ok(n)  => n as i64,
        Err(e) => e.to_errno(),
    }
}

/// `setgroups(size, list_ptr)` — remplace la liste de groupes supplémentaires.
pub fn sys_setgroups(size: u64, list_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if size > 65536 { return E2BIG; }
    if size > 0 && list_ptr == 0 { return EFAULT; }
    match crate::process::core::registry::PROCESS_REGISTRY.set_groups(list_ptr, size as usize) {
        Ok(_)  => 0,
        Err(e) => e.to_errno(),
    }
}

/// `capget(hdrp, datap)` — lit les capabilities POSIX.1e.
pub fn sys_capget(hdrp: u64, datap: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if hdrp == 0 { return EFAULT; }
    // Délégue à security/capability
    match crate::security::capability::capget(hdrp, datap) {
        Ok(_)  => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `capset(hdrp, datap)` — écrit les capabilities.
pub fn sys_capset(hdrp: u64, datap: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if hdrp == 0 || datap == 0 { return EFAULT; }
    match crate::security::capability::capset(hdrp, datap) {
        Ok(_)  => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
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
        SYS_SETUID   => Some(sys_setuid),
        SYS_SETGID   => Some(sys_setgid),
        SYS_SETRESUID => Some(sys_setresuid),
        SYS_GETRESUID => Some(sys_getresuid),
        SYS_SETRESGID => Some(sys_setresgid),
        SYS_GETRESGID => Some(sys_getresgid),
        SYS_SETSID    => Some(sys_setsid),
        SYS_GETSID    => Some(sys_getsid),
        SYS_SETPGID   => Some(sys_setpgid),
        SYS_GETPGID   => Some(sys_getpgid),
        SYS_UMASK     => Some(sys_umask),
        SYS_GETDENTS64 => Some(sys_getdents64),
        SYS_READLINK  => Some(sys_readlink),
        SYS_READLINKAT => Some(sys_readlinkat),
        SYS_TIMES     => Some(sys_times),
        SYS_GETGROUPS => Some(sys_getgroups),
        SYS_SETGROUPS => Some(sys_setgroups),
        SYS_CAPGET    => Some(sys_capget),
        SYS_CAPSET    => Some(sys_capset),
        _             => None,
    }
}
