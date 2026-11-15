//! # syscall/handlers/misc.rs — Miscellaneous thin wrappers (getuid, getgid, uname, sysinfo)
//!
//! RULE SYS-03: THIN WRAPPERS ONLY.
//! ABI-03: FORBIDDEN to return a kernel pointer in rax.

use crate::syscall::errno::{EFAULT, EINVAL, EPERM, ESRCH};
use crate::syscall::validation::USER_ADDR_MAX;

/// `getpid()` -> PID of the current process.
pub fn sys_getpid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // Fast-path has already intercepted getpid — this handler is the fallback.
    let pid_val: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 {
            return 1;
        }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock))
            .pid
            .0
    };
    pid_val as i64
}

/// `getppid()` -> PID of parent.
pub fn sys_getppid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let pid_val: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 {
            return 0;
        }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock))
            .pid
            .0
    };
    let pid = crate::process::core::pid::Pid(pid_val);
    match crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) {
        Some(pcb) => pcb.ppid.load(core::sync::atomic::Ordering::Acquire) as i64,
        None => 0,
    }
}

/// `gettid()` -> TID of current thread.
pub fn sys_gettid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let tid_val: u64 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 {
            return 1;
        }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock)).tid
    };
    tid_val as i64
}

/// `getuid()` -> real UID.
pub fn sys_getuid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // ABI-03: return UID value, not a kernel pointer.
    // Delegates -> process::core::creds::get_uid()
    0 // root UID by default (not yet implemented)
}

/// `getgid()` -> real GID.
pub fn sys_getgid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    0
}

/// `geteuid()` -> effective UID.
pub fn sys_geteuid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    0
}

/// `getegid()` -> effective GID.
pub fn sys_getegid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    0
}

/// `uname(utsname_ptr)` -> 0 or errno.
///
/// Remplit `struct utsname` (6 × 65 bytes = 390 bytes) :
///   sysname[65], nodename[65], release[65], version[65], machine[65], domainname[65]
pub fn sys_uname(buf_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if buf_ptr == 0 || buf_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }

    let mut buf = [0u8; 390];
    macro_rules! write_field {
        ($off:expr, $s:expr) => {{
            let src: &[u8] = $s;
            let len = src.len().min(64);
            buf[$off..$off + len].copy_from_slice(&src[..len]);
        }};
    }

    write_field!(0, b"Exo-OS"); // sysname
    write_field!(65, b"exo-os"); // nodename
    write_field!(130, b"1.0.0"); // release
    write_field!(195, b"#1 SMP 2026"); // version
    write_field!(260, b"x86_64"); // machine
    write_field!(325, b"(none)"); // domainname

    match crate::syscall::validation::write_user_typed(buf_ptr, buf) {
        Ok(()) => 0,
        Err(e) => e.to_errno(),
    }
}

/// `sysinfo(info_ptr)` -> 0 or errno.
pub fn sys_sysinfo(info_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if info_ptr == 0 || info_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
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

    let uptime_ns = crate::scheduler::timer::clock::monotonic_ns();
    let info = SysInfo {
        uptime: (uptime_ns / 1_000_000_000) as i64,
        totalram: crate::memory::physical::stats::total_pages() as u64,
        freeram: crate::memory::physical::stats::free_pages() as u64,
        procs: crate::process::core::registry::PROCESS_REGISTRY.count() as u16,
        mem_unit: 4096,
        ..SysInfo::default()
    };
    match crate::syscall::validation::write_user_typed(info_ptr, info) {
        Ok(()) => 0,
        Err(e) => e.to_errno(),
    }
}

/// `arch_prctl(code, addr)` -> 0 or errno.
///
/// Used to configure FS_BASE (TLS) via ARCH_SET_FS (0x1002).
/// BUG-04 FIX: do_exec() must also call set_fs_base directly (PROC-10).
pub fn sys_arch_prctl(code: u64, addr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    const ARCH_SET_GS: u64 = 0x1001;
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;
    const ARCH_GET_GS: u64 = 0x1004;
    match code {
        ARCH_SET_FS => {
            if !crate::memory::core::address::is_user_canonical(addr) {
                return EINVAL;
            }
            // Update TCB of current thread
            let tcb_ptr: u64;
            unsafe {
                core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
                if tcb_ptr != 0 {
                    let tcb = &mut *(tcb_ptr as *mut crate::scheduler::ThreadControlBlock);
                    tcb.fs_base = addr;
                }
            }
            // Write IA32_FS_BASE MSR — initializes Ring3 TLS.
            unsafe {
                core::arch::x86_64::_mm_mfence();
                // wrmsrl(IA32_FS_BASE=0xC000_0100, addr)
                core::arch::asm!(
                    "wrmsr",
                    in("ecx") 0xC000_0100u32,
                    in("eax") (addr & 0xFFFF_FFFF) as u32,
                    in("edx") (addr >> 32) as u32,
                    options(nomem, nostack),
                );
            }
            0
        }
        ARCH_SET_GS => {
            if !crate::memory::core::address::is_user_canonical(addr) {
                return EINVAL;
            }
            let tcb_ptr: u64;
            unsafe {
                core::arch::asm!("mov {}, gs:[0x20]", out(reg) tcb_ptr, options(nostack, nomem));
                if tcb_ptr != 0 {
                    let tcb = &mut *(tcb_ptr as *mut crate::scheduler::ThreadControlBlock);
                    tcb.user_gs_base = addr;
                }
            }
            unsafe {
                core::arch::asm!(
                    "wrmsr",
                    in("ecx") 0xC000_0102u32,
                    in("eax") (addr & 0xFFFF_FFFF) as u32,
                    in("edx") (addr >> 32) as u32,
                    options(nomem, nostack),
                );
            }
            0
        }
        ARCH_GET_FS => {
            if addr == 0 || addr >= USER_ADDR_MAX {
                return EFAULT;
            }
            let fs_base: u64;
            unsafe {
                let low: u32;
                let high: u32;
                core::arch::asm!(
                    "rdmsr",
                    in("ecx") 0xC000_0100u32,
                    out("eax") low,
                    out("edx") high,
                    options(nomem, nostack),
                );
                fs_base = ((high as u64) << 32) | (low as u64);
            }
            if crate::syscall::validation::write_user_typed::<u64>(addr, fs_base).is_err() {
                return EFAULT;
            }
            0
        }
        ARCH_GET_GS => {
            if addr == 0 || addr >= USER_ADDR_MAX {
                return EFAULT;
            }
            let gs_base: u64;
            unsafe {
                let low: u32;
                let high: u32;
                core::arch::asm!(
                    "rdmsr",
                    in("ecx") 0xC000_0102u32,
                    out("eax") low,
                    out("edx") high,
                    options(nomem, nostack),
                );
                gs_base = ((high as u64) << 32) | (low as u64);
            }
            if crate::syscall::validation::write_user_typed::<u64>(addr, gs_base).is_err() {
                return EFAULT;
            }
            0
        }
        _ => EINVAL,
    }
}

/// `set_tid_address(tidptr)` -> current TID.
pub fn sys_set_tid_address(tidptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if tidptr != 0 && tidptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    // SAFETY: gs:[0x20] holds the current ThreadControlBlock pointer during syscall execution.
    unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr != 0 {
            let tcb = &*(ptr as *const crate::scheduler::core::task::ThreadControlBlock);
            let pid = crate::process::core::pid::Pid(tcb.pid.0);
            let tid = crate::process::core::pid::Tid(tcb.tid as u32);
            if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) {
                let thread_ptr = pcb.find_thread_by_tid(tid);
                if !thread_ptr.is_null() {
                    (*thread_ptr)
                        .clear_child_tid
                        .store(tidptr, core::sync::atomic::Ordering::Release);
                }
            }
            return tcb.tid as i64;
        }
    }
    sys_gettid(0, 0, 0, 0, 0, 0)
}

/// `prctl(option, arg2, arg3, arg4, arg5)`.
pub fn sys_prctl(opt: u64, arg2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    const PR_SET_PDEATHSIG: u64 = 1;
    const PR_GET_PDEATHSIG: u64 = 2;
    const PR_GET_DUMPABLE: u64 = 3;
    const PR_SET_DUMPABLE: u64 = 4;
    const PR_SET_NAME: u64 = 15;
    const PR_GET_NAME: u64 = 16;
    const PR_SET_NO_NEW_PRIVS: u64 = 38;
    const PR_GET_NO_NEW_PRIVS: u64 = 39;

    match opt {
        PR_SET_PDEATHSIG | PR_SET_DUMPABLE | PR_SET_NO_NEW_PRIVS | PR_GET_NO_NEW_PRIVS => 0,
        PR_GET_DUMPABLE => 1,
        PR_GET_PDEATHSIG => {
            if arg2 == 0 || arg2 >= USER_ADDR_MAX {
                return EFAULT;
            }
            if crate::syscall::validation::write_user_typed::<i32>(arg2, 0).is_err() {
                return EFAULT;
            }
            0
        }
        PR_SET_NAME => {
            if arg2 == 0 || arg2 >= USER_ADDR_MAX {
                return EFAULT;
            }
            let mut raw = [0u8; 16];
            for (i, slot) in raw.iter_mut().take(15).enumerate() {
                let addr = arg2.saturating_add(i as u64);
                let Ok(b) = crate::syscall::validation::read_user_typed::<u8>(addr) else {
                    return EFAULT;
                };
                if b == 0 {
                    break;
                }
                *slot = b;
            }
            // SAFETY: gs:[0x20] holds the current ThreadControlBlock pointer during syscall execution.
            unsafe {
                let ptr: u64;
                core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
                if ptr != 0 {
                    let tcb = &*(ptr as *const crate::scheduler::core::task::ThreadControlBlock);
                    let pid = crate::process::core::pid::Pid(tcb.pid.0);
                    let tid = crate::process::core::pid::Tid(tcb.tid as u32);
                    if let Some(pcb) =
                        crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid)
                    {
                        let thread_ptr = pcb.find_thread_by_tid(tid);
                        if !thread_ptr.is_null() {
                            *(*thread_ptr).name.lock() = raw;
                        }
                        if tid.0 == pid.0 {
                            pcb.set_name_bytes(&raw);
                        }
                    }
                }
            }
            0
        }
        PR_GET_NAME => {
            if arg2 == 0 || arg2 >= USER_ADDR_MAX {
                return EFAULT;
            }
            let mut out = [0u8; 16];
            // SAFETY: gs:[0x20] holds the current ThreadControlBlock pointer during syscall execution.
            unsafe {
                let ptr: u64;
                core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
                if ptr != 0 {
                    let tcb = &*(ptr as *const crate::scheduler::core::task::ThreadControlBlock);
                    let pid = crate::process::core::pid::Pid(tcb.pid.0);
                    let tid = crate::process::core::pid::Tid(tcb.tid as u32);
                    if let Some(pcb) =
                        crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid)
                    {
                        let thread_ptr = pcb.find_thread_by_tid(tid);
                        if !thread_ptr.is_null() {
                            out = *(*thread_ptr).name.lock();
                        }
                        if out[0] == 0 {
                            let snap = pcb.name_snapshot();
                            let n = snap.len().min(15);
                            out[..n].copy_from_slice(&snap[..n]);
                        }
                    }
                }
            }
            if crate::syscall::validation::copy_to_user(arg2 as *mut u8, out.as_ptr(), 16).is_err()
            {
                return EFAULT;
            }
            0
        }
        _ => EINVAL,
    }
}

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
            SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle | SchedPolicy::Deadline => {
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
            SchedPolicy::Fifo | SchedPolicy::RoundRobin => posix_rt_prio_from_internal(tcb.priority),
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
        Some(SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle | SchedPolicy::Deadline) => {
            0
        }
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
        Some(SchedPolicy::Normal | SchedPolicy::Batch | SchedPolicy::Idle | SchedPolicy::Deadline) => {
            0
        }
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
