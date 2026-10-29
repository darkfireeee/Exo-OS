//! # syscall/table/memory.rs — Handlers de gestion mémoire (mmap, brk, mprotect, madvise)

use super::common::{checked_i32_sysarg, checked_u32_sysarg, checked_usize_sysarg, stat_inc};
use crate::syscall::numbers::*;
use core::sync::atomic::Ordering;

// ─────────────────────────────────────────────────────────────────────────────
// Handlers Mémoire (délégués vers memory/)
// ─────────────────────────────────────────────────────────────────────────────

/// `mmap(addr, len, prot, flags, fd, off)` → adresse mappée ou errno.
pub fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: u64, off: u64) -> i64 {
    stat_inc(SYS_MMAP);
    // Longueur doit être > 0 et multiple de PAGE_SIZE
    if len == 0 {
        return EINVAL;
    }
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let flags = match checked_u32_sysarg(flags) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let fd = match checked_i32_sysarg(fd) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let _len_pages = len.saturating_add(4095) / 4096;
    // Déléguer à memory/virtual/mmap.rs
    match crate::memory::virt::mmap::do_mmap(addr, len, prot, flags, fd, off) {
        Ok(va) => va as i64,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `munmap(addr, len)`.
pub fn sys_munmap(addr: u64, len: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MUNMAP);
    if len == 0 {
        return EINVAL;
    }
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_munmap(addr, len) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `mprotect(addr, len, prot)`.
pub fn sys_mprotect(addr: u64, len: u64, prot: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_MPROTECT);
    if len == 0 {
        return EINVAL;
    }
    let len = match checked_usize_sysarg(len) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let prot = match checked_u32_sysarg(prot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    match crate::memory::virt::mmap::do_mprotect(addr, len, prot) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `brk(addr)` → nouvelle borne du segment data ou errno.
pub fn sys_brk(addr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_BRK);
    match crate::memory::virt::mmap::do_brk(addr) {
        Ok(new_brk) => {
            sync_current_pcb_brk(new_brk);
            new_brk as i64
        }
        Err(_) => ENOMEM,
    }
}

fn sync_current_pcb_brk(new_brk: u64) {
    let tcb = crate::scheduler::core::switch::current_thread_raw();
    if tcb.is_null() {
        return;
    }

    // SAFETY: current_thread_raw() returned a non-null TCB for the running thread.
    let pid = unsafe { (*tcb).pid.0 };
    if pid == 0 {
        return;
    }

    if let Some(pcb) = crate::process::core::registry::PROCESS_REGISTRY
        .find_by_pid(crate::process::core::pid::Pid(pid))
    {
        pcb.brk_current.store(new_brk, Ordering::Release);
    }
}

