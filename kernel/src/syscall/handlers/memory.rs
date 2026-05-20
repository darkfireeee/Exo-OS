//! # syscall/handlers/memory.rs — Thin wrappers mémoire (mmap, munmap, mprotect, brk)
//!
//! RÈGLE SYS-03 : THIN WRAPPERS UNIQUEMENT.
//! Délègue à memory::virtual::mmap (déjà intégré dans table.rs).

use crate::syscall::errno::{EINVAL, ENOMEM};

/// `mmap(addr, len, prot, flags, fd, off)` → adresse mappée ou errno.
pub fn sys_mmap(addr: u64, len: u64, prot: u64, flags: u64, fd: u64, off: u64) -> i64 {
    if len == 0 {
        return EINVAL;
    }
    match crate::memory::virt::mmap::do_mmap(
        addr,
        len as usize,
        prot as u32,
        flags as u32,
        fd as i32,
        off,
    ) {
        Ok(va) => va as i64,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `munmap(addr, len)` → 0 ou errno.
pub fn sys_munmap(addr: u64, len: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if len == 0 {
        return EINVAL;
    }
    match crate::memory::virt::mmap::do_munmap(addr, len as usize) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `mprotect(addr, len, prot)` → 0 ou errno.
pub fn sys_mprotect(addr: u64, len: u64, prot: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    if len == 0 {
        return EINVAL;
    }
    match crate::memory::virt::mmap::do_mprotect(addr, len as usize, prot as u32) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `brk(addr)` → nouvelle borne du segment data ou errno.
pub fn sys_brk(addr: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
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
        pcb.brk_current
            .store(new_brk, core::sync::atomic::Ordering::Release);
    }
}
