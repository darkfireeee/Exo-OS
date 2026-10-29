//! Primitives mémoire : `mmap` (9), `mprotect` (10), `munmap` (11), `mremap` (25).

use crate::{abi, decode_ret, SysResult};

pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;

pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_FIXED: u64 = 0x10;
pub const MAP_ANONYMOUS: u64 = 0x20;
pub const MREMAP_MAYMOVE: u64 = 1;

#[inline]
pub unsafe fn mmap(
    addr: usize,
    len: usize,
    prot: u64,
    flags: u64,
    fd: i64,
    offset: u64,
) -> SysResult<*mut u8> {
    let ret = unsafe {
        abi::syscall6(
            abi::SYS_MMAP,
            addr as u64,
            len as u64,
            prot,
            flags,
            fd as u64,
            offset,
        )
    };
    decode_ret(ret).map(|p| p as *mut u8)
}

#[inline]
pub unsafe fn mprotect(addr: *mut u8, len: usize, prot: u64) -> SysResult<()> {
    let ret = unsafe { abi::syscall3(abi::SYS_MPROTECT, addr as u64, len as u64, prot) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub unsafe fn munmap(addr: *mut u8, len: usize) -> SysResult<()> {
    let ret = unsafe { abi::syscall2(abi::SYS_MUNMAP, addr as u64, len as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub unsafe fn mremap(
    old_addr: *mut u8,
    old_size: usize,
    new_size: usize,
    flags: u64,
) -> SysResult<*mut u8> {
    let ret = unsafe {
        abi::syscall4(
            abi::SYS_MREMAP,
            old_addr as u64,
            old_size as u64,
            new_size as u64,
            flags,
        )
    };
    decode_ret(ret).map(|p| p as *mut u8)
}
