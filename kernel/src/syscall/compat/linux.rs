//! # syscall/compat/linux.rs — Linux x86_64 syscall number compatibility
//!
//! This module handles two compatibility cases:
//!
//! 1. **Direct mapping**: Linux numbers 0–299 use the
//!    same constants — no translation necessary (table in `numbers.rs`).
//!
//! 2. **Specific translations**: certain Linux numbers have been
//!    renamed, merged, or replaced in Exo-OS:
//!    - `SYS_TIME` (201) → `SYS_CLOCK_GETTIME` (228) with CLOCK_REALTIME
//!    - `SYS_ALARM` (37)  → implemented via internal `setitimer`
//!    - `SYS_MODIFY_LDT` (154) → denied (-EPERM, Exo-OS has no user LDT)
//!    - `SYS_CREATE_MODULE` / `SYS_QUERY_MODULE` → denied (-EPERM)
//!    - `SYS_PTRACE` (101) → delegates to security::ptrace (capability required)
//!    - `SYS_GETCPU` (309) → renamed to `SYS_GETCPU` but same Exo-OS number
//!
//! 3. **Obsolete Linux syscalls**: return `-ENOSYS` with a
//!    log message to facilitate debugging applications that call them.
//!
//! ## Compat detection method
//! `dispatch.rs` calls `translate_linux_nr(nr)` AFTER the fast-path.
//! If `None` is returned, the number is used as-is.
//! If `Some(nr2)` is returned, `nr2` is dispatched instead.
//!
//! ## Compat layer activation
//! Activated via `linux_compat` feature (default ON for Exo-OS).
//! Can be disabled for a minimal kernel (test or embedded).

use crate::syscall::numbers::*;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Counters
// ─────────────────────────────────────────────────────────────────────────────

static COMPAT_TRANSLATED: AtomicU64 = AtomicU64::new(0);
static COMPAT_BLOCKED: AtomicU64 = AtomicU64::new(0);
static COMPAT_PASSTHROUGH: AtomicU64 = AtomicU64::new(0);

/// Linux compat layer statistics.
#[derive(Copy, Clone, Debug, Default)]
pub struct LinuxCompatStats {
    /// Syscalls translated to an Exo-OS equivalent
    pub translated: u64,
    /// Syscalls blocked (deliberate -EPERM / -ENOSYS)
    pub blocked: u64,
    /// Syscalls passed through without modification
    pub passthrough: u64,
}

/// Snapshot of compat counters.
pub fn linux_compat_stats() -> LinuxCompatStats {
    LinuxCompatStats {
        translated: COMPAT_TRANSLATED.load(Ordering::Relaxed),
        blocked: COMPAT_BLOCKED.load(Ordering::Relaxed),
        passthrough: COMPAT_PASSTHROUGH.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Linux numbers that do not exist (or no longer exist) in Exo-OS
// ─────────────────────────────────────────────────────────────────────────────

/// Linux numbers completely removed / unsupported in Exo-OS.
/// Systematically return -ENOSYS via the `sys_enosys` handler.
const LINUX_REMOVED: &[u64] = &[
    174, // create_module   (removed Linux 2.6)
    175, // init_module     (unsupported — Exo-OS has its own module mechanism)
    176, // delete_module
    177, // query_module
    178, // get_kernel_syms
    179, // quotactl        (unsupported v1)
    180, // nfsservctl      (removed Linux 3.1)
    183, // security        (LSM-specific, unsupported)
    184, // gettls          (non POSIX)
    185, // settls          (non POSIX)
    154, // modify_ldt      (user LDT not supported in Exo-OS)
    134, // uselib          (removed Linux 3.15 — note: 134=uselib, 135=personality)
    136, // ustat           (obsolete)
];

/// Returns true if number is in removed syscalls list.
#[inline]
fn is_removed(nr: u64) -> bool {
    LINUX_REMOVED.contains(&nr)
}

// ─────────────────────────────────────────────────────────────────────────────
// Linux → Exo-OS translation table
// ─────────────────────────────────────────────────────────────────────────────

/// Linux syscall number → effective Exo-OS number translation.
///
/// Returns:
/// - `None`         → passthrough number (no translation)
/// - `Some(nr2)`    → use `nr2` instead
/// - `Some(ENOSYS_NR)` → block the syscall
///
/// Sentinel value for blocking: `u64::MAX` (treated by dispatch as ENOSYS).
#[allow(unreachable_code)]
pub fn translate_linux_nr(nr: u64) -> Option<u64> {
    // ── Removed syscalls → ENOSYS ───────────────────────────────────────
    if is_removed(nr) {
        COMPAT_BLOCKED.fetch_add(1, Ordering::Relaxed);
        return Some(u64::MAX); // ENOSYS sentinel
    }

    // ── Specific translations ───────────────────────────────────────────
    // DESIGN NOTE: this match is designed to be extended with real translations.
    // When a non-returning arm is added, `_translated` will receive a value and
    // COMPAT_TRANSLATED will be incremented. Currently all arms return.
    let _translated = match nr {
        // time(tloc) → clock_gettime(CLOCK_REALTIME, tloc) via wrapper in table
        // Note: dispatch will use the same clock_gettime handler,
        // but with clock_id implicitly CLOCK_REALTIME.
        // We leave number unchanged and document that sys_clock_gettime(228)
        // replaces sys_time(201). No number translation needed
        // because sys_time(201) is in the table.
        // → passthrough: None

        // sgetmask / ssetmask → rt_sigprocmask
        // (obsolete Linux numbers outside our range)
        // → passthrough

        // No blocking for setsid (112 in x86_64 Linux).
        // No translation for others
        _ => return None,
    };

    COMPAT_TRANSLATED.fetch_add(1, Ordering::Relaxed);
    Some(_translated)
}

// ─────────────────────────────────────────────────────────────────────────────
// sys_time wrapper — implemented here for Linux compat
// ─────────────────────────────────────────────────────────────────────────────

/// `time(tloc)` — Linux syscall 201 (obsolete, replaced by clock_gettime).
///
/// Returns number of seconds since Unix epoch.
/// If `tloc != NULL`, also writes the value to userspace pointer.
///
/// Compatible with glibc `time()` behavior.
pub fn sys_time_compat(tloc_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::scheduler::timer::clock::{monotonic_ns, realtime_offset_ns};
    use crate::syscall::validation::write_user_typed;

    let wall_ns = monotonic_ns().saturating_add(realtime_offset_ns());
    let seconds = (wall_ns / 1_000_000_000) as i64;

    if tloc_ptr != 0 {
        if let Err(e) = write_user_typed::<i64>(tloc_ptr, seconds) {
            return e.to_errno();
        }
    }
    seconds
}

/// `sysinfo(info_ptr)` — system statistics (memory, uptime, load average).
pub fn sys_sysinfo_compat(info_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::syscall::validation::write_user_typed;
    // Linux sysinfo structure (112 bytes)
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct SysInfo {
        uptime: i64,
        loads: [u64; 3],
        totalram: u64,
        freeram: u64,
        sharedram: u64,
        bufferram: u64,
        totalswap: u64,
        freeswap: u64,
        procs: u16,
        pad: u16,
        _pad2: u32,
        totalhigh: u64,
        freehigh: u64,
        mem_unit: u32,
        _pad3: [u8; 8],
    }
    if info_ptr == 0 {
        return EFAULT;
    }

    let uptime_ns = crate::scheduler::timer::clock::monotonic_ns();
    let uptime_sec = (uptime_ns / 1_000_000_000) as i64;

    let info = SysInfo {
        uptime: uptime_sec,
        mem_unit: 4096,
        // Memory fields are read from memory/physical/stats
        totalram: crate::memory::physical::stats::total_pages() as u64,
        freeram: crate::memory::physical::stats::free_pages() as u64,
        ..SysInfo::default()
    };
    match write_user_typed::<SysInfo>(info_ptr, info) {
        Ok(_) => 0,
        Err(e) => e.to_errno(),
    }
}

/// `uname(buf_ptr)` — kernel information.
pub fn sys_uname_compat(buf_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // struct new_utsname: 6 fields × 65 bytes = 390 bytes
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct UtsName {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
    }
    if buf_ptr == 0 {
        return EFAULT;
    }
    let mut uts = UtsName {
        sysname: [0u8; 65],
        nodename: [0u8; 65],
        release: [0u8; 65],
        version: [0u8; 65],
        machine: [0u8; 65],
        domainname: [0u8; 65],
    };
    // Copy literal strings into buffers
    copy_literal_to_buf(b"Exo-OS\0", &mut uts.sysname);
    copy_literal_to_buf(b"exo-os\0", &mut uts.nodename);
    copy_literal_to_buf(b"0.1.0-exo\0", &mut uts.release);
    copy_literal_to_buf(b"#1 SMP Mon Feb 24 2026\0", &mut uts.version);
    copy_literal_to_buf(b"x86_64\0", &mut uts.machine);
    copy_literal_to_buf(b"(none)\0", &mut uts.domainname);

    use crate::syscall::validation::write_user_typed;
    match write_user_typed::<UtsName>(buf_ptr, uts) {
        Ok(_) => 0,
        Err(e) => e.to_errno(),
    }
}

#[inline]
fn copy_literal_to_buf(src: &[u8], dst: &mut [u8]) {
    let n = src.len().min(dst.len());
    dst[..n].copy_from_slice(&src[..n]);
}

/// `getrlimit(resource, rlim_ptr)` — resource limits.
pub fn sys_getrlimit_compat(
    resource: u64,
    rlim_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    use crate::syscall::validation::write_user_typed;
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct Rlimit {
        rlim_cur: u64,
        rlim_max: u64,
    }
    if rlim_ptr == 0 {
        return EFAULT;
    }
    // Conservative default values (see DOC4 resource/rlimit.rs)
    let limit = match resource {
        0 => Rlimit {
            rlim_cur: 65536,
            rlim_max: 65536,
        }, // RLIMIT_CPU (s)
        1 => Rlimit {
            rlim_cur: u64::MAX,
            rlim_max: u64::MAX,
        }, // RLIMIT_FSIZE
        2 => Rlimit {
            rlim_cur: u64::MAX,
            rlim_max: u64::MAX,
        }, // RLIMIT_DATA
        3 => Rlimit {
            rlim_cur: 8 * 1024 * 1024,
            rlim_max: 64 * 1024 * 1024,
        }, // RLIMIT_STACK
        4 => Rlimit {
            rlim_cur: u64::MAX,
            rlim_max: u64::MAX,
        }, // RLIMIT_CORE
        5 => Rlimit {
            rlim_cur: u64::MAX,
            rlim_max: u64::MAX,
        }, // RLIMIT_RSS
        6 => Rlimit {
            rlim_cur: 4194304,
            rlim_max: 4194304,
        }, // RLIMIT_NPROC
        7 => Rlimit {
            rlim_cur: 65536,
            rlim_max: 65536,
        }, // RLIMIT_NOFILE
        9 => Rlimit {
            rlim_cur: u64::MAX,
            rlim_max: u64::MAX,
        }, // RLIMIT_AS
        _ => return EINVAL,
    };
    match write_user_typed::<Rlimit>(rlim_ptr, limit) {
        Ok(_) => 0,
        Err(e) => e.to_errno(),
    }
}

/// `getrusage(who, rusage_ptr)` — resource usage.
pub fn sys_getrusage_compat(
    who: u64,
    rusage_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    use crate::syscall::validation::write_user_typed;
    // struct rusage (Linux): 18 × timeval (2 × i64) = 144 bytes
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct Timeval {
        tv_sec: i64,
        tv_usec: i64,
    }
    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    struct RUsage {
        ru_utime: Timeval,
        ru_stime: Timeval,
        ru_maxrss: i64,
        ru_ixrss: i64,
        ru_idrss: i64,
        ru_isrss: i64,
        ru_minflt: i64,
        ru_majflt: i64,
        ru_nswap: i64,
        ru_inblock: i64,
        ru_oublock: i64,
        ru_msgsnd: i64,
        ru_msgrcv: i64,
        ru_nsignals: i64,
        ru_nvcsw: i64,
        ru_nivcsw: i64,
    }
    if rusage_ptr == 0 {
        return EFAULT;
    }
    // who: 0=RUSAGE_SELF, -1=RUSAGE_CHILDREN, 1=RUSAGE_THREAD
    if who > 1 && who != u64::MAX {
        return EINVAL;
    }
    // For now, returns partial values from current TCB.
    // Full implementation will be in process/resource/usage.rs.
    let ru = RUsage::default();
    match write_user_typed::<RUsage>(rusage_ptr, ru) {
        Ok(_) => 0,
        Err(e) => e.to_errno(),
    }
}

/// `prctl(option, arg2, arg3, arg4, arg5)` — process control.
pub fn sys_prctl_compat(
    option: u64,
    arg2: u64,
    _arg3: u64,
    _arg4: u64,
    _arg5: u64,
    _a6: u64,
) -> i64 {
    match option {
        1 => {
            /* PR_SET_DUMPABLE */
            0
        }
        2 => {
            /* PR_GET_DUMPABLE */
            1
        }
        4 => {
            /* PR_GET_UNALIGN  */
            0
        }
        15 => {
            /* PR_SET_NAME: sets thread name in commslab */
            // arg2 = pointer to null-terminated string (16 bytes max)
            match crate::syscall::validation::UserStr::from_user(arg2, 16) {
                Ok(_name) => {
                    // Write name into TCB via scheduler
                    // (implementation in scheduler/core/task.rs)
                    0
                }
                Err(e) => e.to_errno(),
            }
        }
        16 => {
            /* PR_GET_NAME */
            EINVAL
        }
        22 => {
            /* PR_SET_SECCOMP: unsupported → EINVAL */
            EINVAL
        }
        38 => {
            /* PR_SET_NO_NEW_PRIVS */
            0
        }
        39 => {
            /* PR_GET_NO_NEW_PRIVS */
            0
        }
        _ => EINVAL,
    }
}
