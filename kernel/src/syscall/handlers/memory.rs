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
        Ok(new_brk) => new_brk as i64,
        Err(_) => ENOMEM,
    }
}
