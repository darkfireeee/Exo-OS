//! # syscall/handlers/sched.rs — POSIX scheduling, priority & CPU affinity wrappers
//!
//! RULE SYS-03: THIN WRAPPERS ONLY.

use crate::syscall::errno::{EFAULT, EINVAL, EPERM, ESRCH};
use crate::syscall::validation::USER_ADDR_MAX;

const PRIO_PROCESS: u64 = 0;

#[repr(C)]
#[derive(Copy, Clone)]
struct SchedParam {
    sched_priority: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct SchedTimespec {
    tv_sec: i64,
    tv_nsec: i64,
}

fn current_tcb_mut() -> Option<&'static mut crate::scheduler::core::task::ThreadControlBlock> {
    let ptr = crate::scheduler::core::switch::current_thread_raw();
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

fn caller_credentials() -> (u32, u32, bool) {
    let Some(cur) = current_tcb_mut() else {
        return (0, u32::MAX, false);
    };
    let pid = cur.pid.0;
    let euid = crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
        .map(|pcb| pcb.euid_fast())
        .unwrap_or(u32::MAX);
    // Root (euid == 0) or core Ring-1 system services (PID 1..=10).
    (pid, euid, euid == 0 || (1..=10).contains(&pid))
}

fn with_target_tcb<R>(
    raw_id: u64,
    f: impl FnOnce(
        &crate::process::core::pcb::ProcessControlBlock,
        &mut crate::scheduler::core::task::ThreadControlBlock,
    ) -> Result<R, i64>,
) -> Result<R, i64> {
    if raw_id > u32::MAX as u64 {
        return Err(ESRCH);
    }
    let target = raw_id as u32;
    if target == 0 {
        let Some(tcb) = current_tcb_mut() else {
            return Err(ESRCH);
        };
        let pid = crate::process::core::pid::Pid(tcb.pid.0);
        let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) else {
            return Err(ESRCH);
        };
        return f(pcb, tcb);
    }

    let mut f_opt = Some(f);
    if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(target))
    {
        let mut tptr = pcb.find_thread_by_tid(crate::process::core::pid::Tid(target));
        if tptr.is_null() {
            tptr = pcb.find_alive_thread();
        }
        if !tptr.is_null() {
            if let Some(cb) = f_opt.take() {
                // SAFETY: thread slot belongs to live PCB in registry.
                let tcb = unsafe { &mut *(*tptr).tcb_ptr() };
                return cb(pcb, tcb);
            }
        }
    }

    let target_tid = crate::process::core::pid::Tid(target);
    let mut out: Option<Result<R, i64>> = None;
    crate::process::core::registry::PROCESS_REGISTRY.for_each(|pcb| {
        if out.is_some() {
            return;
        }
        let tptr = pcb.find_thread_by_tid(target_tid);
        if !tptr.is_null() {
            if let Some(cb) = f_opt.take() {
                // SAFETY: thread slot belongs to live PCB in registry.
                let tcb = unsafe { &mut *(*tptr).tcb_ptr() };
                out = Some(cb(pcb, tcb));
            }
        }
    });
    out.unwrap_or(Err(ESRCH))
}

fn check_modify_permission(
    caller_pid: u32,
    caller_euid: u32,
    caller_priv: bool,
    target_pcb: &crate::process::core::pcb::ProcessControlBlock,
) -> Result<(), i64> {
    if caller_priv || target_pcb.pid.0 == caller_pid || target_pcb.euid_fast() == caller_euid {
        Ok(())
    } else {
        Err(EPERM)
    }
}

fn posix_rt_prio_from_internal(prio: crate::scheduler::core::task::Priority) -> i32 {
    (99i32 - prio.0 as i32).clamp(1, 99)
}

fn internal_rt_prio_from_posix(sched_priority: i32) -> crate::scheduler::core::task::Priority {
    crate::scheduler::core::task::Priority((99i32 - sched_priority).clamp(0, 99) as u8)
}

/// `getpriority(which, who)` -> Linux kernel scale `20 - nice` (`1..=40`) or errno.
pub fn sys_getpriority(which: u64, who: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if which != PRIO_PROCESS {
        return EINVAL;
    }
    match with_target_tcb(who, |_pcb, tcb| Ok(20i64 - (tcb.priority.to_nice() as i64))) {
        Ok(v) => v,
        Err(e) => e,
    }
}

/// `setpriority(which, who, niceval)` -> 0 or errno.
pub fn sys_setpriority(which: u64, who: u64, prio_val: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::scheduler::core::task::{Priority, SchedPolicy};

    if which != PRIO_PROCESS {
        return EINVAL;
    }
    let nice = (prio_val as i32).clamp(-20, 19) as i8;
    let new_prio = Priority::from_nice(nice);
    let (caller_pid, caller_euid, caller_priv) = caller_credentials();

    match with_target_tcb(who, |pcb, tcb| {
        check_modify_permission(caller_pid, caller_euid, caller_priv, pcb)?;
        if !caller_priv && new_prio.0 < tcb.priority.0 {
            return Err(EPERM);
        }
        if !matches!(
            tcb.policy,
            SchedPolicy::Fifo | SchedPolicy::RoundRobin | SchedPolicy::Deadline
        ) {
            tcb.priority = new_prio;
        }
        Ok(0)
    }) {
        Ok(v) => v,
        Err(e) => e,
    }
}

/// `sched_setparam(pid, param_ptr)` -> 0 or errno.
pub fn sys_sched_setparam(pid: u64, param_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::scheduler::core::task::SchedPolicy;

    if param_ptr == 0 || param_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let Ok(param) = crate::syscall::validation::read_user_typed::<SchedParam>(param_ptr) else {
        return EFAULT;
    };
    let (caller_pid, caller_euid, caller_priv) = caller_credentials();

    match with_target_tcb(pid, |pcb, tcb| {
        check_modify_permission(caller_pid, caller_euid, caller_priv, pcb)?;
        match tcb.policy {
            SchedPolicy::Fifo | SchedPolicy::RoundRobin => {
                if !(1..=99).contains(&param.sched_priority) {
                    return Err(EINVAL);
                }
                if !caller_priv {
                    return Err(EPERM);
                }
                tcb.priority = internal_rt_prio_from_posix(param.sched_priority);
            }
            SchedPolicy::Normal
            | SchedPolicy::Batch
            | SchedPolicy::Idle
            | SchedPolicy::Deadline => {
                if param.sched_priority != 0 {
                    return Err(EINVAL);
                }
            }
        }
        let target_cpu = tcb.current_cpu();
        tcb.request_preemption();
        // SAFETY: scheduler per-CPU state is initialized before user syscalls run.
        unsafe {
            crate::scheduler::core::switch::notify_cpu_reschedule(target_cpu.0 as usize);
        }
        Ok(0)
    }) {
        Ok(v) => v,
        Err(e) => e,
    }
}

/// `sched_getparam(pid, param_ptr)` -> 0 or errno.
pub fn sys_sched_getparam(pid: u64, param_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    use crate::scheduler::core::task::SchedPolicy;

    if param_ptr == 0 || param_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let sched_priority = match with_target_tcb(pid, |_pcb, tcb| {
        Ok(match tcb.policy {
            SchedPolicy::Fifo | SchedPolicy::RoundRobin => {
                posix_rt_prio_from_internal(tcb.priority)
            }
            _ => 0,
        })
    }) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let param = SchedParam { sched_priority };
    if crate::syscall::validation::write_user_typed::<SchedParam>(param_ptr, param).is_err() {
        return EFAULT;
    }
    0
}

/// `sched_setscheduler(pid, policy, param_ptr)` -> 0 or errno.
pub fn sys_sched_setscheduler(
    pid: u64,
    policy_u64: u64,
    param_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    use crate::scheduler::core::task::{Priority, SchedPolicy};

    let Some(new_policy) = SchedPolicy::from_posix(policy_u64 as u32) else {
        return EINVAL;
    };
    if param_ptr == 0 || param_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let Ok(param) = crate::syscall::validation::read_user_typed::<SchedParam>(param_ptr) else {
        return EFAULT;
    };

    let is_rt_param = matches!(new_policy, SchedPolicy::Fifo | SchedPolicy::RoundRobin);
    if is_rt_param {
        if !(1..=99).contains(&param.sched_priority) {
            return EINVAL;
        }
    } else if param.sched_priority != 0 {
        return EINVAL;
    }

    let requires_priv = matches!(
        new_policy,
        SchedPolicy::Fifo | SchedPolicy::RoundRobin | SchedPolicy::Deadline
    );
    let (caller_pid, caller_euid, caller_priv) = caller_credentials();
    if requires_priv && !caller_priv {
        return EPERM;
    }

    match with_target_tcb(pid, |pcb, tcb| {
        check_modify_permission(caller_pid, caller_euid, caller_priv, pcb)?;
        tcb.policy = new_policy;
        tcb.priority = match new_policy {
            SchedPolicy::Fifo | SchedPolicy::RoundRobin => {
                internal_rt_prio_from_posix(param.sched_priority)
            }
            SchedPolicy::Idle => Priority::IDLE,
            SchedPolicy::Deadline => Priority::RT_MAX,
            SchedPolicy::Normal | SchedPolicy::Batch => {
                if (Priority::NORMAL_MAX.0..=Priority::NORMAL_MIN.0).contains(&tcb.priority.0) {
                    tcb.priority
                } else {
                    Priority::NORMAL_DEFAULT
                }
            }
        };
        let target_cpu = tcb.current_cpu();
        tcb.request_preemption();
        // SAFETY: scheduler per-CPU state is initialized before user syscalls run.
        unsafe {
            crate::scheduler::core::switch::notify_cpu_reschedule(target_cpu.0 as usize);
        }
        Ok(0)
    }) {
        Ok(v) => v,
        Err(e) => e,
    }
}

/// `sched_getscheduler(pid)` -> POSIX `SCHED_*` constant or errno.
pub fn sys_sched_getscheduler(pid: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    match with_target_tcb(pid, |_pcb, tcb| Ok(tcb.policy.as_posix() as i64)) {
        Ok(v) => v,
        Err(e) => e,
    }
}

/// `sched_get_priority_max(policy)` -> max priority or `EINVAL`.
pub fn sys_sched_get_priority_max(
    policy_u64: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    use crate::scheduler::core::task::SchedPolicy;
    match SchedPolicy::from_posix(policy_u64 as u32) {
        Some(SchedPolicy::Fifo | SchedPolicy::RoundRobin) => 99,
        Some(
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle | SchedPolicy::Deadline,
        ) => 0,
        None => EINVAL,
    }
}

/// `sched_get_priority_min(policy)` -> min priority or `EINVAL`.
pub fn sys_sched_get_priority_min(
    policy_u64: u64,
    _a2: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    use crate::scheduler::core::task::SchedPolicy;
    match SchedPolicy::from_posix(policy_u64 as u32) {
        Some(SchedPolicy::Fifo | SchedPolicy::RoundRobin) => 1,
        Some(
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle | SchedPolicy::Deadline,
        ) => 0,
        None => EINVAL,
    }
}

/// `sched_rr_get_interval(pid, tp_ptr)` -> 0 or errno.
pub fn sys_sched_rr_get_interval(
    pid: u64,
    tp_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    use crate::scheduler::core::task::SchedPolicy;

    if tp_ptr == 0 || tp_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let policy = match with_target_tcb(pid, |_pcb, tcb| Ok(tcb.policy)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let ts = SchedTimespec {
        tv_sec: 0,
        tv_nsec: match policy {
            SchedPolicy::RoundRobin => 100_000_000,
            _ => 0,
        },
    };
    if crate::syscall::validation::write_user_typed::<SchedTimespec>(tp_ptr, ts).is_err() {
        return EFAULT;
    }
    0
}

/// `sched_getaffinity(pid, cpusetsize, mask_ptr)` -> bytes written or errno.
pub fn sys_sched_getaffinity(
    pid: u64,
    cpusetsize: u64,
    mask_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    if cpusetsize < 8 || (cpusetsize & 7) != 0 {
        return EINVAL;
    }
    if mask_ptr == 0 || mask_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let online_bits = crate::scheduler::smp::affinity::CpuSet::full().bits[0];
    let mask = match with_target_tcb(pid, |_pcb, tcb| {
        Ok(tcb.cpu_affinity_mask().bits[0] & online_bits)
    }) {
        Ok(m) => m,
        Err(e) => return e,
    };
    if crate::syscall::validation::write_user_typed::<u64>(mask_ptr, mask).is_err() {
        return EFAULT;
    }
    8
}

/// `sched_setaffinity(pid, cpusetsize, mask_ptr)` -> 0 or errno.
pub fn sys_sched_setaffinity(
    pid: u64,
    cpusetsize: u64,
    mask_ptr: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    if cpusetsize < 8 || (cpusetsize & 7) != 0 {
        return EINVAL;
    }
    if mask_ptr == 0 || mask_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let Ok(mask) = crate::syscall::validation::read_user_typed::<u64>(mask_ptr) else {
        return EFAULT;
    };
    let requested = crate::scheduler::smp::affinity::CpuSet::new([mask, 0, 0, 0])
        .and(&crate::scheduler::smp::affinity::CpuSet::full());
    if requested.is_empty() {
        return EINVAL;
    }
    let (caller_pid, caller_euid, caller_priv) = caller_credentials();
    match with_target_tcb(pid, |pcb, tcb| {
        check_modify_permission(caller_pid, caller_euid, caller_priv, pcb)?;
        tcb.set_cpu_affinity_mask(requested);
        if !requested.test(tcb.current_cpu()) {
            let cur_cpu = tcb.current_cpu();
            tcb.request_preemption();
            // SAFETY: scheduler per-CPU state is initialized before user syscalls run.
            unsafe {
                crate::scheduler::core::switch::notify_cpu_reschedule(cur_cpu.0 as usize);
            }
        }
        Ok(0)
    }) {
        Ok(v) => v,
        Err(e) => e,
    }
}

/// `sched_yield()` -> 0 (yields CPU to next ready thread).
pub fn sys_sched_yield(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    unsafe {
        let tcb_ptr = crate::scheduler::core::switch::current_thread_raw();
        if !tcb_ptr.is_null() {
            let tcb = &mut *tcb_ptr;
            let cpu_id = tcb.current_cpu();
            crate::scheduler::core::switch::schedule_yield(cpu_id, tcb);
        }
    }
    0
}

/// `getcpu(cpu_ptr, node_ptr, tcache)` -> 0 or errno.
pub fn sys_getcpu(cpu_ptr: u64, node_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if cpu_ptr != 0 && cpu_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    if node_ptr != 0 && node_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    let cpu = unsafe {
        let tcb_ptr = crate::scheduler::core::switch::current_thread_raw();
        if tcb_ptr.is_null() {
            0u32
        } else {
            (*tcb_ptr).current_cpu().0
        }
    };
    if cpu_ptr != 0 && crate::syscall::validation::write_user_typed::<u32>(cpu_ptr, cpu).is_err() {
        return EFAULT;
    }
    if node_ptr != 0 && crate::syscall::validation::write_user_typed::<u32>(node_ptr, 0).is_err() {
        return EFAULT;
    }
    0
}
