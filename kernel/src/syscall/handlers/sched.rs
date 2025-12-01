//! Process Scheduling Syscalls
//!
//! Implements sched_yield, nice, setpriority, getpriority, sched_setscheduler, sched_getscheduler, sched_setparam, sched_getparam.

// Scheduling parameter structure
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SchedParam {
    pub sched_priority: i32,
}

// Scheduling policies
pub const SCHED_OTHER: i32 = 0;
pub const SCHED_FIFO: i32 = 1;
pub const SCHED_RR: i32 = 2;
pub const SCHED_BATCH: i32 = 3;
pub const SCHED_IDLE: i32 = 5;

// Priority "which" values
pub const PRIO_PROCESS: i32 = 0;
pub const PRIO_PGRP: i32 = 1;
pub const PRIO_USER: i32 = 2;

/// Yield the CPU to other processes
pub unsafe fn sys_sched_yield() -> i64 {
    log::info!("sys_sched_yield");
    // TODO: Call scheduler to yield
    // For now, just return success
    0
}

/// Adjust process priority (deprecated interface)
pub unsafe fn sys_nice(inc: i32) -> i64 {
    log::info!("sys_nice: inc={}", inc);
    // TODO: Adjust process priority
    // Return current priority
    0
}

/// Set process/group/user priority
pub unsafe fn sys_setpriority(which: i32, who: u32, prio: i32) -> i64 {
    log::info!(
        "sys_setpriority: which={}, who={}, prio={}",
        which,
        who,
        prio
    );
    // TODO: Set priority based on which (PRIO_PROCESS, PRIO_PGRP, PRIO_USER)
    0
}

/// Get process/group/user priority
pub unsafe fn sys_getpriority(which: i32, who: u32) -> i64 {
    log::info!("sys_getpriority: which={}, who={}", which, who);
    // TODO: Get priority based on which
    // Return default priority (nice value 0 = priority 20)
    20
}

/// Set scheduling policy and parameters
pub unsafe fn sys_sched_setscheduler(pid: i32, policy: i32, param: *const SchedParam) -> i64 {
    if !param.is_null() {
        let p = &*param;
        log::info!(
            "sys_sched_setscheduler: pid={}, policy={}, priority={}",
            pid,
            policy,
            p.sched_priority
        );
    } else {
        log::info!(
            "sys_sched_setscheduler: pid={}, policy={}, param=null",
            pid,
            policy
        );
    }
    // TODO: Set scheduling policy
    0
}

/// Get scheduling policy
pub unsafe fn sys_sched_getscheduler(pid: i32) -> i64 {
    log::info!("sys_sched_getscheduler: pid={}", pid);
    // Return SCHED_OTHER as default
    SCHED_OTHER as i64
}

/// Set scheduling parameters
pub unsafe fn sys_sched_setparam(pid: i32, param: *const SchedParam) -> i64 {
    if !param.is_null() {
        let p = &*param;
        log::info!(
            "sys_sched_setparam: pid={}, priority={}",
            pid,
            p.sched_priority
        );
    } else {
        log::info!("sys_sched_setparam: pid={}, param=null", pid);
        return -14; // EFAULT
    }
    // TODO: Set scheduling parameters
    0
}

/// Get scheduling parameters
pub unsafe fn sys_sched_getparam(pid: i32, param: *mut SchedParam) -> i64 {
    if param.is_null() {
        return -14; // EFAULT
    }

    log::info!("sys_sched_getparam: pid={}", pid);

    let p = &mut *param;
    // Return default priority
    p.sched_priority = 0;

    0
}
