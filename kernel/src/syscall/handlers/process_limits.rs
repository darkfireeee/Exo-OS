//! Process Resource Limits System Call Handlers
//!
//! Handles resource limits: getrlimit, setrlimit, prlimit64, getrusage

use crate::memory::{MemoryError, MemoryResult};
use crate::scheduler::SCHEDULER;

/// Resource limit structure
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Rlimit {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

// Resource constants
pub const RLIMIT_CPU: u32 = 0;
pub const RLIMIT_FSIZE: u32 = 1;
pub const RLIMIT_DATA: u32 = 2;
pub const RLIMIT_STACK: u32 = 3;
pub const RLIMIT_CORE: u32 = 4;
pub const RLIMIT_RSS: u32 = 5;
pub const RLIMIT_NPROC: u32 = 6;
pub const RLIMIT_NOFILE: u32 = 7;
pub const RLIMIT_MEMLOCK: u32 = 8;
pub const RLIMIT_AS: u32 = 9;
pub const RLIMIT_LOCKS: u32 = 10;
pub const RLIMIT_SIGPENDING: u32 = 11;
pub const RLIMIT_MSGQUEUE: u32 = 12;
pub const RLIMIT_NICE: u32 = 13;
pub const RLIMIT_RTPRIO: u32 = 14;
pub const RLIMIT_RTTIME: u32 = 15;

pub const RLIM_INFINITY: u64 = !0;

/// Get resource limit
pub fn sys_getrlimit(resource: u32, rlim: *mut Rlimit) -> MemoryResult<()> {
    log::debug!("sys_getrlimit: resource={}", resource);

    if rlim.is_null() {
        return Err(MemoryError::InvalidAddress);
    }

    // TODO: Retrieve actual limits from process structure
    // For now, return infinite/default limits
    let limit = match resource {
        RLIMIT_NOFILE => Rlimit { rlim_cur: 1024, rlim_max: 4096 },
        RLIMIT_STACK => Rlimit { rlim_cur: 8 * 1024 * 1024, rlim_max: RLIM_INFINITY },
        _ => Rlimit { rlim_cur: RLIM_INFINITY, rlim_max: RLIM_INFINITY },
    };

    unsafe {
        *rlim = limit;
    }

    Ok(())
}

/// Set resource limit
pub fn sys_setrlimit(resource: u32, rlim: *const Rlimit) -> MemoryResult<()> {
    log::debug!("sys_setrlimit: resource={}", resource);

    if rlim.is_null() {
        return Err(MemoryError::InvalidAddress);
    }

    let limit = unsafe { *rlim };
    log::info!("setrlimit: resource {} set to cur={}, max={}", resource, limit.rlim_cur, limit.rlim_max);

    // TODO: Store limits in process structure
    // TODO: Check permissions (only root can increase hard limit)

    Ok(())
}

/// Get/Set resource limit (prlimit64)
pub fn sys_prlimit64(
    pid: u64,
    resource: u32,
    new_limit: *const Rlimit,
    old_limit: *mut Rlimit,
) -> MemoryResult<()> {
    log::debug!("sys_prlimit64: pid={}, resource={}", pid, resource);

    // 1. Check permissions if pid != 0
    let target_pid = if pid == 0 {
        SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0)
    } else {
        pid
    };

    // 2. Get old limit if requested
    if !old_limit.is_null() {
        // Reuse sys_getrlimit logic (but for specific pid)
        let limit = match resource {
            RLIMIT_NOFILE => Rlimit { rlim_cur: 1024, rlim_max: 4096 },
            RLIMIT_STACK => Rlimit { rlim_cur: 8 * 1024 * 1024, rlim_max: RLIM_INFINITY },
            _ => Rlimit { rlim_cur: RLIM_INFINITY, rlim_max: RLIM_INFINITY },
        };
        unsafe { *old_limit = limit; }
    }

    // 3. Set new limit if requested
    if !new_limit.is_null() {
        let limit = unsafe { *new_limit };
        log::info!("prlimit64: pid {} resource {} set to cur={}, max={}", target_pid, resource, limit.rlim_cur, limit.rlim_max);
        // TODO: Update process limits
    }

    Ok(())
}

/// Resource usage structure
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Rusage {
    pub ru_utime: crate::syscall::handlers::time::TimeSpec, // User time
    pub ru_stime: crate::syscall::handlers::time::TimeSpec, // System time
    pub ru_maxrss: i64,   // Max resident set size
    pub ru_ixrss: i64,    // Integral shared memory size
    pub ru_idrss: i64,    // Integral unshared data size
    pub ru_isrss: i64,    // Integral unshared stack size
    pub ru_minflt: i64,   // Page reclaims (soft page faults)
    pub ru_majflt: i64,   // Page faults (hard page faults)
    pub ru_nswap: i64,    // Swaps
    pub ru_inblock: i64,  // Block input operations
    pub ru_oublock: i64,  // Block output operations
    pub ru_msgsnd: i64,   // IPC messages sent
    pub ru_msgrcv: i64,   // IPC messages received
    pub ru_nsignals: i64, // Signals received
    pub ru_nvcsw: i64,    // Voluntary context switches
    pub ru_nivcsw: i64,   // Involuntary context switches
}

pub const RUSAGE_SELF: i32 = 0;
pub const RUSAGE_CHILDREN: i32 = -1;
pub const RUSAGE_THREAD: i32 = 1;

/// Get resource usage
pub fn sys_getrusage(who: i32, usage: *mut Rusage) -> MemoryResult<()> {
    log::debug!("sys_getrusage: who={}", who);

    if usage.is_null() {
        return Err(MemoryError::InvalidAddress);
    }

    // TODO: Retrieve actual usage stats from scheduler/process
    let mut ru = Rusage::default();
    
    // Stub values
    ru.ru_utime.seconds = 0;
    ru.ru_utime.nanoseconds = 1000;
    ru.ru_stime.seconds = 0;
    ru.ru_stime.nanoseconds = 500;
    ru.ru_maxrss = 4096; // 4KB

    unsafe {
        *usage = ru;
    }

    Ok(())
}
