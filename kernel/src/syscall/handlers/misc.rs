//! # syscall/handlers/misc.rs — Miscellaneous thin wrappers (getuid, getgid, uname, sysinfo)
//!
//! RULE SYS-03: THIN WRAPPERS ONLY.
//! ABI-03: FORBIDDEN to return a kernel pointer in rax.

use crate::syscall::errno::{EFAULT, EINVAL, ENOSYS};
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
    sys_gettid(0, 0, 0, 0, 0, 0)
}

/// `prctl(option, arg2, arg3, arg4, arg5)`.
pub fn sys_prctl(opt: u64, arg2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    let _ = (opt, arg2);
    ENOSYS
}

/// `sched_yield()` -> 0 (yields CPU to next ready thread).
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
pub fn sys_getcpu(cpu_ptr: u64, _node_ptr: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if cpu_ptr != 0 && cpu_ptr >= USER_ADDR_MAX {
        return EFAULT;
    }
    ENOSYS
}
