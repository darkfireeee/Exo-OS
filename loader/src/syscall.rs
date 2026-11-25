//! Direct `#![no_std]` Linux/Exo-OS syscall primitives for `/lib/ld-exo.so`,
//! backed by canonical constants and `syscall6` from `libs/exo-abi`.

pub use exo_abi::{
    syscall6, ARCH_GET_FS, ARCH_SET_FS, EXO_PLEDGE_QUERY_FLAG, EXO_RESTRICTION_NO_DLOPEN,
    SEEK_END, SEEK_SET, SYS_ARCH_PRCTL, SYS_CLOSE, SYS_EXIT, SYS_EXIT_GROUP, SYS_EXO_LOG,
    SYS_EXO_PLEDGE, SYS_GETTID, SYS_LSEEK, SYS_MMAP, SYS_MPROTECT, SYS_MUNMAP, SYS_OPEN,
    SYS_PREAD64, SYS_READ, SYS_WRITE,
};

pub const PROT_NONE: u32 = exo_abi::PROT_NONE as u32;
pub const PROT_READ: u32 = exo_abi::PROT_READ as u32;
pub const PROT_WRITE: u32 = exo_abi::PROT_WRITE as u32;
pub const PROT_EXEC: u32 = exo_abi::PROT_EXEC as u32;

pub const MAP_SHARED: u32 = exo_abi::MAP_SHARED as u32;
pub const MAP_PRIVATE: u32 = exo_abi::MAP_PRIVATE as u32;
pub const MAP_FIXED: u32 = exo_abi::MAP_FIXED as u32;
pub const MAP_ANONYMOUS: u32 = exo_abi::MAP_ANONYMOUS as u32;

pub const O_RDONLY: u32 = exo_abi::O_RDONLY as u32;
pub const EINTR: i64 = exo_abi::errno::positive::EINTR as i64;

#[inline(always)]
unsafe fn decode(ret: i64) -> Result<usize, i32> {
    if ret < 0 && ret >= -4095 {
        Err((-ret) as i32)
    } else {
        Ok(ret as usize)
    }
}

pub unsafe fn sys_mmap(
    addr: usize,
    len: usize,
    prot: u32,
    flags: u32,
    fd: i32,
    offset: u64,
) -> Result<usize, i32> {
    let ret = unsafe {
        syscall6(
            SYS_MMAP,
            addr as u64,
            len as u64,
            prot as u64,
            flags as u64,
            fd as i64 as u64,
            offset,
        )
    };
    unsafe { decode(ret) }
}

pub unsafe fn sys_munmap(addr: usize, len: usize) -> Result<(), i32> {
    let ret = unsafe { syscall6(SYS_MUNMAP, addr as u64, len as u64, 0, 0, 0, 0) };
    unsafe { decode(ret).map(|_| ()) }
}

pub unsafe fn sys_mprotect(addr: usize, len: usize, prot: u32) -> Result<(), i32> {
    let ret = unsafe { syscall6(SYS_MPROTECT, addr as u64, len as u64, prot as u64, 0, 0, 0) };
    unsafe { decode(ret).map(|_| ()) }
}

pub unsafe fn sys_open_ro(path_cstr: *const u8) -> Result<i32, i32> {
    let ret = unsafe { syscall6(SYS_OPEN, path_cstr as u64, O_RDONLY as u64, 0, 0, 0, 0) };
    unsafe { decode(ret).map(|fd| fd as i32) }
}

pub unsafe fn sys_close(fd: i32) -> Result<(), i32> {
    let ret = unsafe { syscall6(SYS_CLOSE, fd as i64 as u64, 0, 0, 0, 0, 0) };
    unsafe { decode(ret).map(|_| ()) }
}

pub unsafe fn sys_lseek(fd: i32, offset: i64, whence: i32) -> Result<usize, i32> {
    let ret = unsafe { syscall6(SYS_LSEEK, fd as i64 as u64, offset as u64, whence as u64, 0, 0, 0) };
    unsafe { decode(ret) }
}

pub unsafe fn sys_pread_exact(fd: i32, buf: &mut [u8], mut offset: u64) -> Result<(), i32> {
    let mut done = 0usize;
    while done < buf.len() {
        let ret = unsafe {
            syscall6(
                SYS_PREAD64,
                fd as i64 as u64,
                buf.as_mut_ptr().add(done) as u64,
                (buf.len() - done) as u64,
                offset,
                0,
                0,
            )
        };
        if ret < 0 {
            let err = (-ret) as i32;
            if err as i64 == EINTR {
                continue;
            }
            return Err(err);
        }
        if ret == 0 {
            return Err(5); // EIO (unexpected EOF)
        }
        let n = ret as usize;
        done += n;
        offset += n as u64;
    }
    Ok(())
}

pub unsafe fn sys_arch_set_fs(fs_base: u64) -> Result<(), i32> {
    let ret = unsafe { syscall6(SYS_ARCH_PRCTL, ARCH_SET_FS, fs_base, 0, 0, 0, 0) };
    unsafe { decode(ret).map(|_| ()) }
}

pub unsafe fn sys_arch_get_fs() -> Result<u64, i32> {
    let mut out = 0u64;
    let ret = unsafe {
        syscall6(
            SYS_ARCH_PRCTL,
            ARCH_GET_FS,
            &mut out as *mut u64 as u64,
            0,
            0,
            0,
            0,
        )
    };
    unsafe { decode(ret).map(|_| out) }
}

pub fn sys_log(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = syscall6(SYS_EXO_LOG, bytes.as_ptr() as u64, bytes.len() as u64, 0, 0, 0, 0);
    }
}

/// Queries the caller's active `SYS_EXO_PLEDGE` restriction bitmask (read-only).
/// Returns `0` when unsupported (e.g. host unit test builds).
pub unsafe fn sys_exo_pledge_query() -> u64 {
    let ret = unsafe { syscall6(SYS_EXO_PLEDGE, 0, EXO_PLEDGE_QUERY_FLAG, 0, 0, 0, 0) };
    if ret < 0 {
        0
    } else {
        ret as u64
    }
}

pub fn sys_exit_group(code: i32) -> ! {
    unsafe {
        let _ = syscall6(SYS_EXIT_GROUP, code as i64 as u64, 0, 0, 0, 0, 0);
        let _ = syscall6(SYS_EXIT, code as i64 as u64, 0, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
