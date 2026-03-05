//! # syscall/handlers/misc.rs — Thin wrappers divers (getuid, getgid, uname, sysinfo)
//!
//! RÈGLE SYS-03 : THIN WRAPPERS UNIQUEMENT.
//! ABI-03 : INTERDIT de retourner un pointeur kernel dans rax.

#![allow(dead_code)]

use crate::syscall::validation::USER_ADDR_MAX;
use crate::syscall::errno::{EFAULT, ENOSYS, EINVAL};

/// `getpid()` → PID du processus courant.
pub fn sys_getpid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // Fast-path a déjà intercepté getpid — ce handler est le fallback.
    let pid_val: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 { return 1; }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock)).pid.0
    };
    pid_val as i64
}

/// `getppid()` → PID du parent.
pub fn sys_getppid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let pid_val: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 { return 0; }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock)).pid.0
    };
    let pid = crate::process::core::pid::Pid(pid_val);
    match crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid) {
        Some(pcb) => pcb.ppid.load(core::sync::atomic::Ordering::Acquire) as i64,
        None      => 0,
    }
}

/// `gettid()` → TID du thread courant.
pub fn sys_gettid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let tid_val: u32 = unsafe {
        let ptr: u64;
        core::arch::asm!("mov {}, gs:[0x20]", out(reg) ptr, options(nomem, nostack));
        if ptr == 0 { return 1; }
        (*(ptr as *const crate::scheduler::core::task::ThreadControlBlock)).tid.0
    };
    tid_val as i64
}

/// `getuid()` → UID réel.
pub fn sys_getuid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // ABI-03 : retourner la valeur UID, pas un pointeur kernel.
    // Délègue → process::core::creds::get_uid()
    0 // root UID par défaut (not yet implemented)
}

/// `getgid()` → GID réel.
pub fn sys_getgid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    0
}

/// `geteuid()` → UID effectif.
pub fn sys_geteuid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    0
}

/// `getegid()` → GID effectif.
pub fn sys_getegid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    0
}

/// `uname(utsname_ptr)` → 0 ou errno.
pub fn sys_uname(buf_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if buf_ptr == 0 || buf_ptr >= USER_ADDR_MAX { return EFAULT; }
    // Délègue → misc::uname::fill_utsname(buf_ptr)
    let _ = buf_ptr;
    ENOSYS
}

/// `sysinfo(info_ptr)` → 0 ou errno.
pub fn sys_sysinfo(info_ptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if info_ptr == 0 || info_ptr >= USER_ADDR_MAX { return EFAULT; }
    let _ = info_ptr;
    ENOSYS
}

/// `arch_prctl(code, addr)` → 0 ou errno.
///
/// Utilisé pour configurer FS_BASE (TLS) via ARCH_SET_FS (0x1002).
/// BUG-04 FIX : do_exec() doit aussi appeler set_fs_base directement (PROC-10).
pub fn sys_arch_prctl(code: u64, addr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    const ARCH_SET_GS: u64 = 0x1001;
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;
    const ARCH_GET_GS: u64 = 0x1004;
    match code {
        ARCH_SET_FS => {
            // Écrire IA32_FS_BASE MSR — initialise le TLS Ring3.
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
        ARCH_GET_FS | ARCH_GET_GS => {
            if addr == 0 || addr >= USER_ADDR_MAX { return EFAULT; }
            ENOSYS
        }
        _ => EINVAL,
    }
}

/// `set_tid_address(tidptr)` → TID courant.
pub fn sys_set_tid_address(tidptr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if tidptr != 0 && tidptr >= USER_ADDR_MAX { return EFAULT; }
    sys_gettid(0, 0, 0, 0, 0, 0)
}

/// `prctl(option, arg2, arg3, arg4, arg5)`.
pub fn sys_prctl(opt: u64, arg2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let _ = (opt, arg2);
    ENOSYS
}

/// `sched_yield()` → 0 (cède le CPU au prochain thread prêt).
pub fn sys_sched_yield(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    unsafe {
        let tcb_ptr = crate::scheduler::core::switch::current_thread_raw();
        if !tcb_ptr.is_null() {
            let tcb = &mut *tcb_ptr;
            let cpu_id = tcb.current_cpu();
            if (cpu_id.0 as usize) < crate::scheduler::core::preempt::MAX_CPUS {
                let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
                crate::scheduler::core::switch::schedule_yield(rq, tcb);
            }
        }
    }
    0
}

/// `getcpu(cpu_ptr, node_ptr, tcache)`.
pub fn sys_getcpu(cpu_ptr: u64, node_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if cpu_ptr != 0 && cpu_ptr >= USER_ADDR_MAX { return EFAULT; }
    ENOSYS
}
