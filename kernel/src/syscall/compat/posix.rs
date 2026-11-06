//! # syscall/compat/posix.rs — POSIX.1-2017 syscall compliance
//!
//! This module centralizes:
//! 1. Missing POSIX constants from header files.
//! 2. Complementary POSIX wrappers (functions that exist in POSIX
//!    whose implementation is spread across multiple kernel modules).
//! 3. Conversion helpers between Exo-OS types and POSIX ABI types.
//! 4. Validation of POSIX arguments (e.g. `whence` values for lseek).
//!
//! ## POSIX syscalls covered by this module
//! - `getgroups` / `setgroups`
//! - `setuid` / `setgid` / `setresuid` / `setresgid` and variants
//! - `umask`
//! - `setsid` / `getsid`
//! - `setpgid` / `getpgid`
//! - `times`
//! - `getdents64`
//! - `readlink` / `readlinkat`
//!
//! Mathematically simple syscalls (getuid, getgid, etc.) reside in
//! `fast_path.rs`. This module handles those requiring data
//! structures or logic.
//!
//! ## POSIX Reference
//! POSIX.1-2017 (IEEE Std 1003.1-2017)

use crate::process::core::pcb::process_flags;
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::syscall::fast_path::syscall_current_pid;
use crate::syscall::numbers::*;
use crate::syscall::validation::{read_user_typed, write_user_typed, SyscallError};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Counter
// ─────────────────────────────────────────────────────────────────────────────

static POSIX_CALL_COUNT: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn inc_posix() {
    POSIX_CALL_COUNT.fetch_add(1, Ordering::Relaxed);
}
/// Returns the number of POSIX syscalls processed by this module.
pub fn posix_call_count() -> u64 {
    POSIX_CALL_COUNT.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// POSIX constants — Flags open(2) / lseek(2) / mmap(2) / etc.
// ─────────────────────────────────────────────────────────────────────────────

/// Flags `open(2)` / `openat(2)` — POSIX.1-2017 compliant + Linux extras.
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
    /// Mask of all allowed flags (for validate_flags)
    // FIX-SYSCALL-OPENFLAGS-01 : includes O_DIRECTORY|O_NOFOLLOW|O_NOATIME|O_CLOEXEC|O_SYNC|O_DIRECT|O_LARGEFILE|O_PATH|O_TMPFILE (T9-008)
    pub const ALLOWED_MASK: u64 = 0x007F_FFFFu64;
}

/// `whence` values for `lseek(2)`.
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
    pub const MAP_POPULATE: u32 = 0x0000_8000;
    pub const MAP_NONBLOCK: u32 = 0x0001_0000;
    pub const MAP_STACK: u32 = 0x0002_0000;
    pub const MAP_HUGETLB: u32 = 0x0004_0000;
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

/// Signals POSIX (identical to Linux for glibc compatibility).
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
    /// First real-time signal
    pub const SIGRTMIN: u32 = 32;
    /// Last real-time signal
    pub const SIGRTMAX: u32 = 64;
}

// ─────────────────────────────────────────────────────────────────────────────
// POSIX validation helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Validates `open()` / `openat()` flags.
/// Returns cleaned flags or EINVAL if reserved bits are set.
#[inline]
pub fn validate_open_flags(flags: u32) -> Result<u32, SyscallError> {
    // Bits beyond ALLOWED_MASK are reserved → EINVAL
    if (flags as u64) & !open_flags::ALLOWED_MASK != 0 {
        return Err(SyscallError::Invalid);
    }
    // O_RDONLY=0 is valid even if value is 0
    Ok(flags)
}

/// Validates mmap protections.
#[inline]
pub fn validate_prot(prot: u32) -> Result<u32, SyscallError> {
    use prot_flags::*;
    let known =
        PROT_NONE | PROT_READ | PROT_WRITE | PROT_EXEC | PROT_SEM | PROT_GROWSDOWN | PROT_GROWSUP;
    if prot & !known != 0 {
        return Err(SyscallError::Invalid);
    }
    if prot & PROT_WRITE != 0 && prot & PROT_EXEC != 0 {
        return Err(SyscallError::Access);
    }
    Ok(prot)
}

/// Validates `mmap()` flags.
#[inline]
pub fn validate_mmap_flags(flags: u32) -> Result<u32, SyscallError> {
    // MAP_SHARED XOR MAP_PRIVATE is required
    let shared = flags & mmap_flags::MAP_SHARED != 0;
    let private = flags & mmap_flags::MAP_PRIVATE != 0;
    if shared == private {
        // Neither or both → EINVAL
        return Err(SyscallError::Invalid);
    }
    Ok(flags)
}

/// Validates `lseek()` `whence` value.
#[inline]
pub fn validate_lseek_whence(whence: u64) -> Result<u32, SyscallError> {
    match whence {
        0 | 1 | 2 | 3 | 4 => Ok(whence as u32),
        _ => Err(SyscallError::Invalid),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// POSIX handlers — Identity / groups
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
            // POSIX.1-2017: root updates real, effective and saved uid.
            // Non-privileged: updates only effective uid if uid32 == uid or suid.
            if c.is_root() {
                pcb.set_uid(uid32);
                pcb.set_euid(uid32);
                pcb.creds.lock().suid = uid32;
                0
            } else if uid32 == c.uid || uid32 == c.suid {
                pcb.set_euid(uid32);
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
            // POSIX.1-2017: root updates real, effective and saved gid.
            // Non-privileged: updates only effective gid if gid32 == gid ou sgid.
            if c.is_root() {
                pcb.set_gid(gid32);
                pcb.set_egid(gid32);
                pcb.creds.lock().sgid = gid32;
                0
            } else if gid32 == c.gid || gid32 == c.sgid {
                pcb.set_egid(gid32);
                0
            } else {
                EPERM
            }
        }
    }
}

/// `setresuid(ruid, euid, suid)` — POSIX + Linux.
/// Value -1 (u32::MAX) = do not modify this field.
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
            // Each value must be u32::MAX (do not change), or one of current IDs, or 0 if root.
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

/// `getresuid(ruid_ptr, euid_ptr, suid_ptr)` — writes the 3 UIDs to userspace.
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
/// Value u32::MAX = do not modify this field.
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

/// `getresgid(rgid_ptr, egid_ptr, sgid_ptr)` — writes the 3 GIDs to userspace.
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

/// `setsid()` — creates a new session for the current process.
/// Fails if process is already group leader (POSIX.1-2017).
pub fn sys_setsid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    let caller = Pid(syscall_current_pid());
    match PROCESS_REGISTRY.find_by_pid(caller) {
        None => EINVAL,
        Some(pcb) => {
            if pcb.is_pgroup_leader() {
                return EPERM; // Fails if already process group leader
            }
            let new_sid = caller.0;
            pcb.set_session_id(new_sid);
            pcb.set_pgroup_id(new_sid);
            // SAFETY: fetch_or on AtomicU32 — no alloc, no missing rights.
            pcb.flags
                .fetch_or(process_flags::SESSION_LEADER, Ordering::Release);
            new_sid as i64
        }
    }
}

/// `getsid(pid)` — returns the SID of process `pid` (or current process if pid=0).
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

/// `setpgid(pid, pgid)` — delegates to process::group::pgrp::setpgid.
pub fn sys_setpgid(pid: u64, pgid: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    use crate::process::core::pid::Pid;
    use crate::process::group::pgrp::{setpgid, PgId};
    let caller = syscall_current_pid();
    let target_pid = if pid == 0 { caller } else { pid as u32 };
    match setpgid(Pid(target_pid), PgId(pgid as u32)) {
        Ok(_) => 0,
        Err(_) => EINVAL,
    }
}

/// `getpgid(pid)` — returns the PGID of process `pid` (or current process if pid=0).
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
// POSIX handlers — umask, times, getdents64
// ─────────────────────────────────────────────────────────────────────────────

/// `umask(mask)` — modifies file creation mask.
/// Note: PCB does not yet store a dedicated `umask` field.
/// This implementation returns requested mask (partial stub, better than ENOSYS).
/// Note Task-5: add `umask: AtomicU32` in PCB and store real value.
pub fn sys_umask(mask: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    // Mask always limited to 9 file permission bits.
    (mask & 0o777) as i64
}

/// `getdents64(fd, dirp, count)` — wired when activating fs/.
pub fn sys_getdents64(fd: u64, dirp: u64, count: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    crate::syscall::table::sys_getdents64(fd, dirp, count, 0, 0, 0)
}

/// `readlink(path, buf, bufsize)` — wired when activating fs/.
pub fn sys_readlink(
    path_ptr: u64,
    buf_ptr: u64,
    bufsize: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    inc_posix();
    crate::syscall::table::sys_readlink(path_ptr, buf_ptr, bufsize, 0, 0, 0)
}

/// `readlinkat(dirfd, path, buf, bufsize)` — wired when activating fs/.
pub fn sys_readlinkat(
    dirfd: u64,
    path_ptr: u64,
    buf_ptr: u64,
    bufsize: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    inc_posix();
    crate::syscall::table::sys_readlinkat(dirfd, path_ptr, buf_ptr, bufsize, 0, 0)
}

/// `times(tbuf)` — returns consumed CPU times.
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
    // POSIX CLK_TCK = 100: one tick is 10 ms.
    let ticks = crate::scheduler::timer::clock::monotonic_ns() / 10_000_000;
    ticks as i64
}

// ─────────────────────────────────────────────────────────────────────────────
// POSIX handlers — Group identification
// ─────────────────────────────────────────────────────────────────────────────

/// `getgroups(size, list_ptr)` — wired during credentials integration.
pub fn sys_getgroups(size: u64, list_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if size != 0 && list_ptr == 0 {
        return EFAULT;
    }
    0
}

/// `setgroups(size, list_ptr)` — wired during credentials integration.
pub fn sys_setgroups(size: u64, list_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if size != 0 && list_ptr == 0 {
        return EFAULT;
    }
    if size == 0 {
        return 0;
    }
    EPERM
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct LinuxCapHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
struct LinuxCapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// `capget(hdrp, datap)` — wired during security/capability integration.
pub fn sys_capget(hdrp: u64, datap: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if hdrp == 0 {
        return EFAULT;
    }
    let _hdr = match read_user_typed::<LinuxCapHeader>(hdrp) {
        Ok(hdr) => hdr,
        Err(_) => return EFAULT,
    };
    if datap != 0 {
        let empty = [LinuxCapData::default(); 2];
        if write_user_typed::<[LinuxCapData; 2]>(datap, empty).is_err() {
            return EFAULT;
        }
    }
    0
}

/// `capset(hdrp, datap)` — wired during security/capability integration.
pub fn sys_capset(hdrp: u64, datap: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    if hdrp == 0 || datap == 0 {
        return EFAULT;
    }
    let _hdr = match read_user_typed::<LinuxCapHeader>(hdrp) {
        Ok(hdr) => hdr,
        Err(_) => return EFAULT,
    };
    let data = match read_user_typed::<[LinuxCapData; 2]>(datap) {
        Ok(data) => data,
        Err(_) => return EFAULT,
    };
    let mut requested = 0u32;
    let mut idx = 0usize;
    while idx < data.len() {
        requested |= data[idx].effective | data[idx].permitted | data[idx].inheritable;
        idx += 1;
    }
    if requested == 0 {
        0
    } else {
        EPERM
    }
}

/// `personality(persona)` — Linux syscall 135 (x86_64 compat).
/// Returns 0 (standard PER_LINUX).
pub fn sys_personality(_persona: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    inc_posix();
    0
}

// ─────────────────────────────────────────────────────────────────────────────
// Export helpers: number → POSIX handler mapping table
// ─────────────────────────────────────────────────────────────────────────────

use crate::syscall::table::SyscallHandler;

/// Returns the POSIX handler for a given number, or `None` if not handled here.
///
/// Called from `table::get_handler()` in addition to the main match.
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
        SYS_PERSONALITY => Some(sys_personality),
        _ => None,
    }
}
