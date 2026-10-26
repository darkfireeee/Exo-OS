use crate::{errno::Errno, sys, Result};

fn with_c_path<R>(path: &str, f: impl FnOnce(*const u8) -> Result<R>) -> Result<R> {
    if path.len() >= 4096 {
        return Err(Errno::EINVAL);
    }
    let mut buf = [0u8; 4096];
    buf[..path.len()].copy_from_slice(path.as_bytes());
    buf[path.len()] = 0;
    f(buf.as_ptr())
}

pub fn open(path: &str, flags: u64, mode: u64) -> Result<i32> {
    with_c_path(path, |c_path| {
        let ret = unsafe { sys::syscall3(sys::SYS_OPEN, c_path as u64, flags, mode) };
        sys::cvt(ret).map(|fd| fd as i32)
    })
}

pub fn close(fd: i32) -> Result<()> {
    let ret = unsafe { sys::syscall1(sys::SYS_CLOSE, fd as u64) };
    sys::cvt(ret).map(|_| ())
}

pub fn read(fd: i32, buf: &mut [u8]) -> Result<usize> {
    let ret = unsafe {
        sys::syscall3(
            sys::SYS_READ,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    sys::cvt(ret)
}

pub fn write(fd: i32, buf: &[u8]) -> Result<usize> {
    let ret = unsafe {
        sys::syscall3(
            sys::SYS_WRITE,
            fd as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
        )
    };
    sys::cvt(ret)
}

pub fn mkdir(path: &str, mode: u64) -> Result<()> {
    with_c_path(path, |c_path| {
        let ret = unsafe {
            sys::syscall3(
                sys::SYS_MKDIRAT,
                sys::AT_FDCWD as u64,
                c_path as u64,
                mode,
            )
        };
        sys::cvt(ret).map(|_| ())
    })
}

pub fn unlink(path: &str) -> Result<()> {
    with_c_path(path, |c_path| {
        let ret = unsafe {
            sys::syscall3(
                sys::SYS_UNLINKAT,
                sys::AT_FDCWD as u64,
                c_path as u64,
                0,
            )
        };
        sys::cvt(ret).map(|_| ())
    })
}
