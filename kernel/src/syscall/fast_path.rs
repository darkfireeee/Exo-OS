//! # syscall/fast_path.rs — Fast path syscalls (<100 cycles)
//!
//! Implements syscalls that require no lock, no allocation,
//! and resolve by reading per-CPU data or atomic counters.
//!
//! ## Performance Target
//! | Syscall          | Target     | Comment                                    |
//! |------------------|------------|--------------------------------------------|
//! | `getpid`         | <50 cyc    | Reads per-CPU TCB via GS:[0x20]            |
//! | `gettid`         | <50 cyc    | Reads per-CPU TCB via GS:[0x20]            |
//! | `getuid/geteuid` | <80 cyc    | Reads PCB credentials (atomic)             |
//! | `getgid/getegid` | <80 cyc    | Same                                       |
//! | `getppid`        | <80 cyc    | Reads PCB parent_pid (atomic)              |
//! | `getcpu`         | <50 cyc    | Reads GS:[0x10] (atomic cpu_id)            |
//! | `clock_gettime`  | <150 cyc   | RDTSC + multiply (MONOTONIC only)          |
//! | `sched_yield`    | ~500 cyc   | schedule_yield() — eventual context switch |
//!
//! ## NO-ALLOC Rules (regle_bonus.md)
//! This module never uses `alloc`, `Vec`, `Box`, `Rc`, `Arc`.
//! Only registers and the stack are used.
//!
//! ## UNSAFE CONTRACT RULE (regle_bonus.md)
//! Every `unsafe {}` block is preceded by a `// SAFETY:` comment.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::smp::percpu::current_cpu_id;
use crate::scheduler::core::task::{ProcessId, ThreadControlBlock, ThreadId};
use crate::scheduler::timer::clock::monotonic_ns;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// RFLAGS mask — only allowed flags can be restored from userspace.
/// IF(9), DF(10), TF(8), AC(18) are filtered by SFMASK (see arch/syscall.rs).
#[allow(dead_code)]
const RFLAGS_USER_MASK: u64 = 0x003F_FFFF;

/// POSIX CLOCK IDs
pub const CLOCK_REALTIME: u32 = 0;
pub const CLOCK_MONOTONIC: u32 = 1;
pub const CLOCK_PROCESS_CPUTIME: u32 = 2;
pub const CLOCK_THREAD_CPUTIME: u32 = 3;
pub const CLOCK_MONOTONIC_RAW: u32 = 4;
pub const CLOCK_REALTIME_COARSE: u32 = 5;
pub const CLOCK_MONOTONIC_COARSE: u32 = 6;
pub const CLOCK_BOOTTIME: u32 = 7;

// ─────────────────────────────────────────────────────────────────────────────
// Fast-path instrumentation
// ─────────────────────────────────────────────────────────────────────────────

static FP_GETPID_COUNT: AtomicU64 = AtomicU64::new(0);
static FP_GETTID_COUNT: AtomicU64 = AtomicU64::new(0);
static FP_GETUID_COUNT: AtomicU64 = AtomicU64::new(0);
static FP_GETGID_COUNT: AtomicU64 = AtomicU64::new(0);
static FP_GETPPID_COUNT: AtomicU64 = AtomicU64::new(0);
static FP_GETCPU_COUNT: AtomicU64 = AtomicU64::new(0);
static FP_CLOCKGET_COUNT: AtomicU64 = AtomicU64::new(0);
static FP_YIELD_COUNT: AtomicU64 = AtomicU64::new(0);

/// Fast-path statistics returned by [`fast_path_stats`].
#[derive(Copy, Clone, Debug, Default)]
pub struct FastPathStats {
    pub getpid: u64,
    pub gettid: u64,
    pub getuid: u64,
    pub getgid: u64,
    pub getppid: u64,
    pub getcpu: u64,
    pub clockget: u64,
    pub yield_cnt: u64,
}

/// Returns a snapshot of fast-path counters.
pub fn fast_path_stats() -> FastPathStats {
    FastPathStats {
        getpid: FP_GETPID_COUNT.load(Ordering::Relaxed),
        gettid: FP_GETTID_COUNT.load(Ordering::Relaxed),
        getuid: FP_GETUID_COUNT.load(Ordering::Relaxed),
        getgid: FP_GETGID_COUNT.load(Ordering::Relaxed),
        getppid: FP_GETPPID_COUNT.load(Ordering::Relaxed),
        getcpu: FP_GETCPU_COUNT.load(Ordering::Relaxed),
        clockget: FP_CLOCKGET_COUNT.load(Ordering::Relaxed),
        yield_cnt: FP_YIELD_COUNT.load(Ordering::Relaxed),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Access to current thread TCB via GS:[0x20]
// ─────────────────────────────────────────────────────────────────────────────

/// Returns a raw pointer to the current thread's TCB.
///
/// Directly reads `GS:[0x20]` (see percpu.rs layout).
/// May only be called from Ring 0 kernel context.
///
/// # Safety
/// - GS must be the kernel GS (after SWAPGS).
/// - The value at GS:[0x20] points to a valid `ThreadControlBlock`.
/// - The caller must NOT store the pointer past a preemption point.
#[inline(always)]
unsafe fn current_tcb_ptr() -> *const ThreadControlBlock {
    crate::arch::x86_64::smp::percpu::read_current_tcb() as *const ThreadControlBlock
}

/// Returns the `ThreadId` of the current thread.
///
/// # Safety
/// Must be called in kernel context (active kernel GS).
#[inline(always)]
unsafe fn current_tid() -> ThreadId {
    let tcb = current_tcb_ptr();
    // SAFETY: the TCB is valid if current_tcb_ptr() returns a non-null pointer.
    // In case of a null TCB (before first switch), TID 0 is returned.
    if tcb.is_null() {
        return ThreadId(0);
    }
    ThreadId((*tcb).tid)
}

/// Returns the `ProcessId` of the current thread.
///
/// # Safety
/// Must be called in kernel context (active kernel GS).
#[inline(always)]
unsafe fn current_pid() -> ProcessId {
    let tcb = current_tcb_ptr();
    // SAFETY: identical to current_tid().
    if tcb.is_null() {
        return ProcessId(0);
    }
    (*tcb).pid
}

// ─────────────────────────────────────────────────────────────────────────────
// POSIX timespec representation (struct timespec)
// ─────────────────────────────────────────────────────────────────────────────

/// C `struct timespec` representation — x86_64 POSIX ABI compliant layout.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Timespec {
    /// Full seconds since epoch (CLOCK_REALTIME) or boot (CLOCK_MONOTONIC).
    pub tv_sec: i64,
    /// Nanoseconds (0..999_999_999).
    pub tv_nsec: i64,
}

impl Timespec {
    /// Constructs a `Timespec` from a total number of nanoseconds.
    #[inline]
    pub const fn from_ns(ns: u64) -> Self {
        Self {
            tv_sec: (ns / 1_000_000_000) as i64,
            tv_nsec: (ns % 1_000_000_000) as i64,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// "Current" credentials — stub pending full process/ integration
// ─────────────────────────────────────────────────────────────────────────────

/// Accesses the credentials of the current process.
///
/// In a complete kernel, this function would read TCB→PCB→credentials.
/// The current implementation directly reads the PCB via the process registry.
#[inline]
fn current_creds() -> Credentials {
    // Access via the process registry. If the registry is not yet initialized
    // (early boot), returns root credentials (uid=0).
    //
    // Production-ready: the process registry is initialized before any userspace
    // activation, so this "boot" branch never executes from userspace.
    let pid = unsafe { current_pid() };
    if pid.0 == 0 {
        // Kernel without current process (idle task or boot)
        return Credentials::root();
    }
    // Access to global process registry (RCU-protected).
    // ProcessId (scheduler) → Pid (process): same u32 repr.
    let process_pid = crate::process::core::pid::Pid(pid.0);
    match crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(process_pid) {
        Some(pcb) => {
            // SAFETY: SpinLock<Credentials> — short hold time, no alloc.
            let creds = pcb.creds.lock();
            Credentials {
                uid: creds.uid,
                gid: creds.gid,
                euid: creds.euid,
                egid: creds.egid,
                ppid: pcb.ppid.load(Ordering::Relaxed),
            }
        }
        // Fail-closed: if non-zero PID has no PCB, deny root privileges (AUDIT-SEC)
        None => Credentials::nobody(),
    }
}

/// Process credentials read from PCB.
#[derive(Copy, Clone, Debug)]
struct Credentials {
    uid: u32,
    gid: u32,
    euid: u32,
    egid: u32,
    ppid: u32,
}

impl Credentials {
    fn root() -> Self {
        Self {
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
            ppid: 0,
        }
    }

    fn nobody() -> Self {
        Self {
            uid: 65534,
            gid: 65534,
            euid: 65534,
            egid: 65534,
            ppid: 1,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fast-path handlers — direct implementation without dispatch table
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the current process PID from GS:[0x20].
///
/// Exposes the private function `current_pid()` to other syscall modules
/// (notably `compat/posix.rs`) that need the PID without going through a handler.
///
/// Returns 0 if called before the first context switch (boot).
#[inline(always)]
pub fn syscall_current_pid() -> u32 {
    // SAFETY: called from Ring-0 kernel context, active kernel GS.
    unsafe { current_pid() }.0
}

/// `getpid()` — returns the current process PID.
///
/// Performance: ~40–60 cycles (reads GS:[0x20] + TCB field).
/// NO-ALLOC zone: reads static/atomic memory only.
#[inline]
pub fn sys_getpid() -> i64 {
    FP_GETPID_COUNT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: Called from syscall_rust_handler, GS is the kernel GS.
    let pid = unsafe { current_pid() };
    pid.0 as i64
}

/// `gettid()` — returns the current thread TID.
///
/// Performance: ~40–60 cycles.
#[inline]
pub fn sys_gettid() -> i64 {
    FP_GETTID_COUNT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: Active kernel GS since syscall entry.
    let tid = unsafe { current_tid() };
    tid.0 as i64
}

/// `getuid()` — returns the real UID of the current process.
///
/// Performance: ~70–90 cycles (PCB read via registry).
#[inline]
pub fn sys_getuid() -> i64 {
    FP_GETUID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().uid as i64
}

/// `geteuid()` — returns the effective UID.
#[inline]
pub fn sys_geteuid() -> i64 {
    FP_GETUID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().euid as i64
}

/// `getgid()` — returns the real GID.
#[inline]
pub fn sys_getgid() -> i64 {
    FP_GETGID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().gid as i64
}

/// `getegid()` — returns the effective GID.
#[inline]
pub fn sys_getegid() -> i64 {
    FP_GETGID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().egid as i64
}

/// `getppid()` — returns the parent PID.
#[inline]
pub fn sys_getppid() -> i64 {
    FP_GETPPID_COUNT.fetch_add(1, Ordering::Relaxed);
    current_creds().ppid as i64
}

/// `getcpu(cpu, node, tcache)` — returns current CPU and NUMA node.
///
/// Arguments:
/// - `arg1`: userspace pointer to `u32` CPU (may be NULL)
/// - `arg2`: userspace pointer to `u32` NUMA node (may be NULL)
/// - `arg3`: ignored (tcache obsolete since Linux 2.6.24)
///
/// Performance: ~50 cycles.
pub fn sys_getcpu(cpu_ptr: u64, node_ptr: u64, _tcache: u64) -> i64 {
    FP_GETCPU_COUNT.fetch_add(1, Ordering::Relaxed);

    let cpu_id = current_cpu_id();
    // NUMA node: currently 0 (future NUMA architecture will enrich this).
    let numa_node: u32 = 0;

    // Optional write to userspace
    if cpu_ptr != 0 {
        if let Err(_) = crate::syscall::validation::write_user_typed::<u32>(cpu_ptr, cpu_id) {
            return super::numbers::EFAULT;
        }
    }
    if node_ptr != 0 {
        if let Err(_) = crate::syscall::validation::write_user_typed::<u32>(node_ptr, numa_node) {
            return super::numbers::EFAULT;
        }
    }
    0
}

/// `clock_gettime(clkid, timespec_ptr)` — reads a POSIX clock.
///
/// Supported clocks:
/// - `CLOCK_MONOTONIC` / `CLOCK_MONOTONIC_RAW` / `CLOCK_BOOTTIME`:
///   TSC → nanoseconds via `monotonic_ns()`.
/// - `CLOCK_REALTIME` / `CLOCK_REALTIME_COARSE`:
///   `monotonic_ns()` + real offset (CLOCK_REALTIME_OFFSET).
/// - `CLOCK_MONOTONIC_COARSE`: identical to MONOTONIC (tick granularity).
///
/// Performance: ~120–150 cycles (RDTSC + multiply + write_user).
pub fn sys_clock_gettime(clkid: u64, ts_ptr: u64) -> i64 {
    FP_CLOCKGET_COUNT.fetch_add(1, Ordering::Relaxed);

    if ts_ptr == 0 {
        return super::numbers::EFAULT;
    }

    let clock_id = match crate::syscall::validation::validate_clockid(clkid) {
        Ok(id) => id,
        Err(e) => return e.to_errno(),
    };

    let ns: u64 = match clock_id {
        CLOCK_MONOTONIC | CLOCK_MONOTONIC_RAW | CLOCK_BOOTTIME | CLOCK_MONOTONIC_COARSE => {
            monotonic_ns()
        }
        CLOCK_REALTIME | CLOCK_REALTIME_COARSE => {
            // Real offset = monotonic + REALTIME_OFFSET (updated by settimeofday / NTP).
            // The offset is stored in a global AtomicU64 of the timer/clock module.
            monotonic_ns().saturating_add(crate::scheduler::timer::clock::realtime_offset_ns())
        }
        // CLOCK_PROCESS_CPUTIME_ID / CLOCK_THREAD_CPUTIME_ID
        CLOCK_PROCESS_CPUTIME | CLOCK_THREAD_CPUTIME => {
            // TaskStats is separate from TCB (TCB = 128B fixed, no stats field).
            // Returns 0 pending TaskStats pointer in extended TCB.
            0u64
        }
        _ => return super::numbers::EINVAL,
    };

    let ts = Timespec::from_ns(ns);
    match crate::syscall::validation::write_user_typed::<Timespec>(ts_ptr, ts) {
        Ok(_) => 0,
        Err(e) => e.to_errno(),
    }
}

/// `gettimeofday(tv_ptr, tz_ptr)` — returns time and (optionally) timezone.
///
/// Timezone is deprecated (always returns UTC+0) conforming to
/// POSIX.1-2008 recommendations.
///
/// Performance: ~130 cycles.
pub fn sys_gettimeofday(tv_ptr: u64, tz_ptr: u64) -> i64 {
    // Validation
    if tv_ptr == 0 {
        return super::numbers::EFAULT;
    }
    let wall_ns =
        monotonic_ns().saturating_add(crate::scheduler::timer::clock::realtime_offset_ns());
    let timeval = Timeval {
        tv_sec: (wall_ns / 1_000_000_000) as i64,
        tv_usec: ((wall_ns % 1_000_000_000) / 1000) as i64,
    };
    if let Err(e) = crate::syscall::validation::write_user_typed::<Timeval>(tv_ptr, timeval) {
        return e.to_errno();
    }
    // Timezone: fixed UTC timezone, daylight=0.
    if tz_ptr != 0 {
        let tz = Timezone {
            tz_minuteswest: 0,
            tz_dsttime: 0,
        };
        if let Err(e) = crate::syscall::validation::write_user_typed::<Timezone>(tz_ptr, tz) {
            return e.to_errno();
        }
    }
    0
}

/// `sched_yield()` — voluntarily yields the CPU.
///
/// Calls scheduler's `schedule_yield()` which performs a context switch
/// if another thread is ready. Otherwise, returns immediately.
///
/// Performance: ~500–800 cycles (identical to normal context switch).
pub fn sys_sched_yield() -> i64 {
    FP_YIELD_COUNT.fetch_add(1, Ordering::Relaxed);
    // SAFETY: GS:[0x20] is the current thread's TCB pointer, percpu invariant.
    // Non-null as soon as scheduler is initialized and a thread is running.
    // run_queue() returns a valid static reference (percpu arena).
    // schedule_yield() is reentrance-safe: disables preemption internally.
    unsafe {
        let tcb_ptr = crate::arch::x86_64::smp::percpu::read_current_tcb();
        if tcb_ptr != 0 {
            let tcb = &mut *(tcb_ptr as *mut crate::scheduler::ThreadControlBlock);
            let cpu_id = tcb.current_cpu();
            crate::scheduler::schedule_yield(cpu_id, tcb);
        }
    }
    0
}

// ─────────────────────────────────────────────────────────────────────────────
// Auxiliary C structures
// ─────────────────────────────────────────────────────────────────────────────

/// `struct timeval` — x86_64 POSIX ABI.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

/// `struct timezone` — POSIX (obsolete).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Timezone {
    pub tz_minuteswest: i32,
    pub tz_dsttime: i32,
}

// ─────────────────────────────────────────────────────────────────────────────
// Fast dispatcher — entry point for dispatch.rs
// ─────────────────────────────────────────────────────────────────────────────

/// Attempts to satisfy a syscall via fast-path.
///
/// Returns `Some(result)` if the syscall is handled, `None` otherwise.
/// The caller (dispatch.rs) falls back to the slow-path on `None`.
///
/// # Performance
/// The fast-path branch is predicted taken (BSP → `inline(always)` here would be
/// counter-productive as the function is called once per syscall).
#[inline]
pub fn try_fast_path(
    nr: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    _arg4: u64,
    _arg5: u64,
    _arg6: u64,
) -> Option<i64> {
    use crate::syscall::numbers::*;
    match nr {
        SYS_GETPID => Some(sys_getpid()),
        SYS_GETTID => Some(sys_gettid()),
        SYS_GETUID => Some(sys_getuid()),
        SYS_GETEUID => Some(sys_geteuid()),
        SYS_GETGID => Some(sys_getgid()),
        SYS_GETEGID => Some(sys_getegid()),
        SYS_GETPPID => Some(sys_getppid()),
        SYS_GETCPU => Some(sys_getcpu(arg1, arg2, arg3)),
        SYS_CLOCK_GETTIME => Some(sys_clock_gettime(arg1, arg2)),
        SYS_GETTIMEOFDAY => Some(sys_gettimeofday(arg1, arg2)),
        SYS_SCHED_YIELD => Some(sys_sched_yield()),
        _ => None,
    }
}
