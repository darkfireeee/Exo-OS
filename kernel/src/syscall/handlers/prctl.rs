//! # syscall/handlers/prctl.rs — Thread TLS, TID address & process control wrappers
//!
//! RULE SYS-03: THIN WRAPPERS ONLY.

use crate::arch::x86_64::cpu::msr::{read_msr, write_msr, MSR_FS_BASE, MSR_KERNEL_GS_BASE};
use crate::arch::x86_64::memory_barrier;
use crate::arch::x86_64::smp::percpu::read_current_tcb;
use crate::syscall::errno::{EFAULT, EINVAL, EPERM};
use crate::syscall::validation::USER_ADDR_MAX;

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
            // Update TCB and ProcessThread of current thread
            unsafe {
                let tcb_ptr = read_current_tcb();
                if tcb_ptr != 0 {
                    let tcb = &mut *(tcb_ptr as *mut crate::scheduler::ThreadControlBlock);
                    tcb.fs_base = addr;
                    let pid = crate::process::core::pid::Pid(tcb.pid.0);
                    let tid = crate::process::core::pid::Tid(tcb.tid as u32);
                    if let Some(pcb) =
                        crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid)
                    {
                        let thread_ptr = pcb.find_thread_by_tid(tid);
                        if !thread_ptr.is_null() {
                            let thread = &mut *thread_ptr;
                            thread.addresses.tls_base = addr;
                            thread
                                .tls_gs_base
                                .store(addr, core::sync::atomic::Ordering::Release);
                        }
                    }
                }
                memory_barrier();
                write_msr(MSR_FS_BASE, addr);
            }
            0
        }
        ARCH_SET_GS => {
            if !crate::memory::core::address::is_user_canonical(addr) {
                return EINVAL;
            }
            unsafe {
                let tcb_ptr = read_current_tcb();
                if tcb_ptr != 0 {
                    let tcb = &mut *(tcb_ptr as *mut crate::scheduler::ThreadControlBlock);
                    tcb.user_gs_base = addr;
                }
                write_msr(MSR_KERNEL_GS_BASE, addr);
            }
            0
        }
        ARCH_GET_FS => {
            if addr == 0 || addr >= USER_ADDR_MAX {
                return EFAULT;
            }
            let fs_base = unsafe { read_msr(MSR_FS_BASE) };
            if crate::syscall::validation::write_user_typed::<u64>(addr, fs_base).is_err() {
                return EFAULT;
            }
            0
        }
        ARCH_GET_GS => {
            if addr == 0 || addr >= USER_ADDR_MAX {
                return EFAULT;
            }
            let gs_base = unsafe { read_msr(MSR_KERNEL_GS_BASE) };
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
        let ptr = read_current_tcb();
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
    super::creds_info::sys_gettid(0, 0, 0, 0, 0, 0)
}

/// FIX-25: Reserved Ring 1 server names that an unprivileged (`ServiceClass::Unknown`)
/// process MUST NOT spoof via `prctl(PR_SET_NAME)`.
pub fn is_reserved_service_name(raw: &[u8]) -> bool {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let mut norm = [0u8; 16];
    let n = end.min(16);
    for i in 0..n {
        norm[i] = if raw[i] == b'-' { b'_' } else { raw[i] };
    }
    let s = &norm[..n];
    matches!(
        s,
        b"init_server"
            | b"ipc_router"
            | b"memory_server"
            | b"vfs_server"
            | b"crypto_server"
            | b"device_server"
            | b"network_server"
            | b"scheduler_serve"
            | b"scheduler_server"
            | b"input_server"
            | b"fb_server"
            | b"tty_server"
            | b"exo_shield"
            | b"audio_server"
            | b"usb_server"
            | b"virtio_drivers"
            | b"exo_compositor"
            | b"exosh"
    )
}

#[inline]
fn current_pcb_for_prctl() -> Option<(u32, u32, &'static crate::process::core::pcb::ProcessControlBlock)> {
    // SAFETY: gs:[0x20] holds the current ThreadControlBlock pointer during syscall execution.
    let ptr = unsafe { read_current_tcb() };
    if ptr == 0 {
        return None;
    }
    let tcb = unsafe { &*(ptr as *const crate::scheduler::core::task::ThreadControlBlock) };
    let pid = tcb.pid.0;
    let tid = tcb.tid as u32;
    let pcb = crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))?;
    Some((pid, tid, pcb))
}

/// `prctl(option, arg2, arg3, arg4, arg5)`.
pub fn sys_prctl(opt: u64, arg2: u64, a3: u64, a4: u64, a5: u64, _a6: u64) -> i64 {
    const PR_SET_PDEATHSIG: u64 = 1;
    const PR_GET_PDEATHSIG: u64 = 2;
    const PR_GET_DUMPABLE: u64 = 3;
    const PR_SET_DUMPABLE: u64 = 4;
    const PR_SET_NAME: u64 = 15;
    const PR_GET_NAME: u64 = 16;
    const PR_SET_NO_NEW_PRIVS: u64 = 38;
    const PR_GET_NO_NEW_PRIVS: u64 = 39;

    match opt {
        PR_SET_NO_NEW_PRIVS => {
            // Linux ABI: arg2 must be 1, arg3..arg5 must be 0; monotonic (irreversible).
            if arg2 != 1 || a3 != 0 || a4 != 0 || a5 != 0 {
                return EINVAL;
            }
            if let Some((_, _, pcb)) = current_pcb_for_prctl() {
                pcb.set_no_new_privs();
            }
            0
        }
        PR_GET_NO_NEW_PRIVS => {
            if arg2 != 0 || a3 != 0 || a4 != 0 || a5 != 0 {
                return EINVAL;
            }
            match current_pcb_for_prctl() {
                Some((_, _, pcb)) => i64::from(pcb.has_no_new_privs()),
                None => 0,
            }
        }
        PR_SET_DUMPABLE => {
            if arg2 > 1 {
                return EINVAL;
            }
            if let Some((_, _, pcb)) = current_pcb_for_prctl() {
                pcb.set_dumpable(arg2 == 1);
            }
            0
        }
        PR_GET_DUMPABLE => match current_pcb_for_prctl() {
            Some((_, _, pcb)) => i64::from(pcb.is_dumpable()),
            None => 1,
        },
        PR_SET_PDEATHSIG => {
            if arg2 > 64 {
                return EINVAL;
            }
            if let Some((_, _, pcb)) = current_pcb_for_prctl() {
                pcb.set_pdeathsig(arg2 as u32);
            }
            0
        }
        PR_GET_PDEATHSIG => {
            if arg2 == 0 || arg2 >= USER_ADDR_MAX {
                return EFAULT;
            }
            let sig = current_pcb_for_prctl()
                .map(|(_, _, pcb)| pcb.pdeathsig() as i32)
                .unwrap_or(0);
            if crate::syscall::validation::write_user_typed::<i32>(arg2, sig).is_err() {
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
            if let Some((pid, tid, pcb)) = current_pcb_for_prctl() {
                // FIX-25: Prevent Ring 3 from forging reserved Ring 1 server names.
                if pid > 1
                    && crate::security::service_class_of(crate::process::core::pid::Pid(pid))
                        == crate::security::ServiceClass::Unknown
                    && is_reserved_service_name(&raw)
                {
                    return EPERM;
                }
                let thread_ptr = pcb.find_thread_by_tid(crate::process::core::pid::Tid(tid));
                if !thread_ptr.is_null() {
                    // SAFETY: thread_ptr belongs to live PCB in PROCESS_REGISTRY.
                    *unsafe { &*thread_ptr }.name.lock() = raw;
                }
                if tid == pid {
                    pcb.set_name_bytes(&raw);
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
                let ptr = read_current_tcb();
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

#[cfg(test)]
mod fix25_prctl_and_personality_tests {
    use super::is_reserved_service_name;
    use crate::syscall::compat::posix::validate_personality_request;
    use crate::syscall::errno::EPERM;

    #[test]
    fn var17_pr_set_name_blocks_ring1_service_impersonation() {
        assert!(is_reserved_service_name(b"exo_shield\0"));
        assert!(is_reserved_service_name(b"exo-shield\0"));
        assert!(is_reserved_service_name(b"crypto_server\0"));
        assert!(is_reserved_service_name(b"init_server\0"));
        assert!(is_reserved_service_name(b"vfs_server\0"));
        assert!(!is_reserved_service_name(b"my_user_app\0"));
    }

    #[test]
    fn var17_personality_blocks_read_implies_exec_and_unpriv_no_aslr() {
        const PER_QUERY: u64 = 0xFFFF_FFFF;
        const ADDR_NO_RANDOMIZE: u64 = 0x0004_0000;
        const READ_IMPLIES_EXEC: u64 = 0x0040_0000;

        assert_eq!(validate_personality_request(PER_QUERY, false, false), Ok(None));
        assert_eq!(
            validate_personality_request(READ_IMPLIES_EXEC, true, false),
            Err(EPERM)
        );
        assert_eq!(
            validate_personality_request(READ_IMPLIES_EXEC, false, false),
            Err(EPERM)
        );
        assert_eq!(
            validate_personality_request(ADDR_NO_RANDOMIZE, false, false),
            Err(EPERM)
        );
        assert_eq!(
            validate_personality_request(ADDR_NO_RANDOMIZE, true, true),
            Err(EPERM)
        );
        assert_eq!(
            validate_personality_request(ADDR_NO_RANDOMIZE, true, false),
            Ok(Some(ADDR_NO_RANDOMIZE as u32))
        );
    }
}
