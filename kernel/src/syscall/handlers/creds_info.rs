//! # syscall/handlers/creds_info.rs — Process identity & system info thin wrappers
//!
//! RULE SYS-03: THIN WRAPPERS ONLY.
//! ABI-03: FORBIDDEN to return a kernel pointer in rax.

use crate::arch::x86_64::smp::percpu::read_current_tcb;
use crate::syscall::errno::EFAULT;
use crate::syscall::validation::USER_ADDR_MAX;

/// `getpid()` -> PID of the current process.
pub fn sys_getpid(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    // Fast-path has already intercepted getpid — this handler is the fallback.
    let pid_val: u32 = unsafe {
        let ptr = read_current_tcb();
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
        let ptr = read_current_tcb();
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
        let ptr = read_current_tcb();
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
    0
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
/// Fills `struct utsname` (6 * 65 bytes = 390 bytes):
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

    write_field!(0, b"Exo-OS");
    write_field!(65, b"exo-os");
    write_field!(130, b"1.0.0");
    write_field!(195, b"#1 SMP 2026");
    write_field!(260, b"x86_64");
    write_field!(325, b"(none)");

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
