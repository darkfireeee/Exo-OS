//! Memory Management Syscalls

use crate::memory::address::VirtualAddress;
use crate::posix_x::kernel_interface::memory_bridge;

/// mmap - Map memory
pub fn sys_mmap(addr: usize, length: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> i64 {
    let vaddr = VirtualAddress::new(addr);

    match memory_bridge::posix_mmap(vaddr, length, prot as u32, flags as u32, fd, offset) {
        Ok(mapped_addr) => mapped_addr.value() as i64,
        Err(errno) => -(errno as i64),
    }
}

/// munmap - Unmap memory
pub fn sys_munmap(addr: usize, length: usize) -> i64 {
    let vaddr = VirtualAddress::new(addr);

    match memory_bridge::posix_munmap(vaddr, length) {
        Ok(()) => 0,
        Err(errno) => -(errno as i64),
    }
}

/// mprotect - Change memory protection
pub fn sys_mprotect(addr: usize, length: usize, prot: i32) -> i64 {
    let vaddr = VirtualAddress::new(addr);

    match memory_bridge::posix_mprotect(vaddr, length, prot as u32) {
        Ok(()) => 0,
        Err(errno) => -(errno as i64),
    }
}

/// brk - Change data segment size
pub fn sys_brk(addr: usize) -> i64 {
    let vaddr = VirtualAddress::new(addr);

    match memory_bridge::posix_brk(vaddr) {
        Ok(new_brk) => new_brk.value() as i64,
        Err(errno) => -(errno as i64),
    }
}
