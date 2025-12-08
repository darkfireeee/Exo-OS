//! Memory Bridge - POSIX to Exo-OS Memory Subsystem
//!
//! Bridges POSIX memory operations to Exo-OS memory management

use crate::memory::address::VirtualAddress;
use crate::memory::{MemoryError, MemoryResult};
use crate::posix_x::translation::errno::{memory_error_to_errno, Errno};

/// Memory protection flags (POSIX)
pub const PROT_NONE: u32 = 0;
pub const PROT_READ: u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC: u32 = 4;

/// Memory mapping flags (POSIX)
pub const MAP_SHARED: u32 = 0x01;
pub const MAP_PRIVATE: u32 = 0x02;
pub const MAP_FIXED: u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;

/// Bridge for memory allocation (brk/sbrk)
pub fn posix_brk(addr: VirtualAddress) -> Result<VirtualAddress, Errno> {
    // Call Exo-OS syscall handler for brk
    match crate::syscall::handlers::memory::sys_brk(addr) {
        Ok(new_brk) => Ok(new_brk),
        Err(e) => Err(memory_error_to_errno(e)),
    }
}

/// Bridge for memory mapping (mmap)
pub fn posix_mmap(
    addr: VirtualAddress,
    length: usize,
    prot: u32,
    flags: u32,
    fd: i32,
    offset: i64,
) -> Result<VirtualAddress, Errno> {
    // Convert POSIX prot flags to ProtFlags
    let prot_flags = crate::syscall::handlers::memory::ProtFlags {
        read: (prot & PROT_READ) != 0,
        write: (prot & PROT_WRITE) != 0,
        exec: (prot & PROT_EXEC) != 0,
    };

    // Convert POSIX flags to MapFlags
    let map_flags = crate::syscall::handlers::memory::MapFlags {
        shared: (flags & MAP_SHARED) != 0,
        private: (flags & MAP_PRIVATE) != 0,
        fixed: (flags & MAP_FIXED) != 0,
        anonymous: (flags & MAP_ANONYMOUS) != 0,
    };

    // Call Exo-OS syscall handler
    match crate::syscall::handlers::memory::sys_mmap(
        addr,
        length,
        prot_flags,
        map_flags,
        fd as u64,
        offset as usize,
    ) {
        Ok(mapped_addr) => Ok(mapped_addr),
        Err(e) => Err(memory_error_to_errno(e)),
    }
}

/// Bridge for memory unmapping (munmap)
pub fn posix_munmap(addr: VirtualAddress, length: usize) -> Result<(), Errno> {
    // Call Exo-OS syscall handler
    match crate::syscall::handlers::memory::sys_munmap(addr, length) {
        Ok(()) => Ok(()),
        Err(e) => Err(memory_error_to_errno(e)),
    }
}

/// Bridge for memory protection (mprotect)
pub fn posix_mprotect(addr: VirtualAddress, length: usize, prot: u32) -> Result<(), Errno> {
    // Convert POSIX prot flags to ProtFlags
    let prot_flags = crate::syscall::handlers::memory::ProtFlags {
        read: (prot & PROT_READ) != 0,
        write: (prot & PROT_WRITE) != 0,
        exec: (prot & PROT_EXEC) != 0,
    };

    // Call Exo-OS syscall handler
    match crate::syscall::handlers::memory::sys_mprotect(addr, length, prot_flags) {
        Ok(()) => Ok(()),
        Err(e) => Err(memory_error_to_errno(e)),
    }
}

/// Initialize memory bridge
pub fn init() {
    log::debug!("Memory bridge initialized");
}
