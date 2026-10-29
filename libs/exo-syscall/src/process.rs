//! Primitives processus, threads, horloge, futex et pledge.

use crate::{abi, decode_ret, SysError, SysResult};

pub use abi::ExoProcessInfo;

pub const PLEDGE_STDIO: u32 = 1 << 0;
pub const PLEDGE_FILESYSTEM: u32 = 1 << 1;
pub const PLEDGE_INET: u32 = 1 << 2;
pub const PLEDGE_IPC: u32 = 1 << 3;
pub const PLEDGE_PROC: u32 = 1 << 4;
pub const PLEDGE_EXEC: u32 = 1 << 5;
pub const PLEDGE_THREAD: u32 = 1 << 6;
pub const PLEDGE_SIGNAL: u32 = 1 << 7;
pub const PLEDGE_ANON: u32 = 1 << 8;
pub const PLEDGE_DEVICE: u32 = 1 << 9;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

pub const CLOCK_REALTIME: u64 = 0;
pub const CLOCK_MONOTONIC: u64 = 1;

pub const FUTEX_WAIT: u32 = 0;
pub const FUTEX_WAKE: u32 = 1;
pub const FUTEX_PRIVATE_FLAG: u32 = 128;

#[inline]
pub fn getpid() -> u32 {
    unsafe { abi::syscall0(abi::SYS_GETPID) as u32 }
}

#[inline]
pub fn getppid() -> u32 {
    unsafe { abi::syscall0(abi::SYS_GETPPID) as u32 }
}

#[inline]
pub fn gettid() -> u32 {
    unsafe { abi::syscall0(abi::SYS_GETTID) as u32 }
}

#[inline]
pub fn sched_yield() {
    let _ = unsafe { abi::syscall0(abi::SYS_SCHED_YIELD) };
}

#[inline]
pub fn exit_group(code: i32) -> ! {
    unsafe {
        let _ = abi::syscall1(abi::SYS_EXIT_GROUP, code as u64);
        let _ = abi::syscall1(abi::SYS_EXIT, code as u64);
    }
    loop {
        core::hint::spin_loop();
    }
}

#[inline]
pub fn fork() -> SysResult<u32> {
    let ret = unsafe { abi::syscall0(abi::SYS_FORK) };
    decode_ret(ret).map(|pid| pid as u32)
}

#[inline]
pub unsafe fn clone_raw(
    flags: u64,
    child_stack: usize,
    ptid: *mut u32,
    ctid: *mut u32,
    tls: usize,
) -> SysResult<u32> {
    let ret = unsafe {
        abi::syscall5(
            abi::SYS_CLONE,
            flags,
            child_stack as u64,
            ptid as u64,
            ctid as u64,
            tls as u64,
        )
    };
    decode_ret(ret).map(|tid| tid as u32)
}

#[inline]
pub unsafe fn execve(
    path_cstr: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> SysResult<()> {
    // RÈGLE PROC-08 : argv ne doit jamais être NULL.
    if path_cstr.is_null() || argv.is_null() {
        return Err(SysError::EFAULT);
    }
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_EXECVE,
            path_cstr as u64,
            argv as u64,
            envp as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn wait4(pid: i32, status: &mut i32, options: i32) -> SysResult<u32> {
    let ret = unsafe {
        abi::syscall4(
            abi::SYS_WAIT4,
            pid as i64 as u64,
            status as *mut i32 as u64,
            options as u64,
            0,
        )
    };
    decode_ret(ret).map(|p| p as u32)
}

#[inline]
pub fn kill(pid: i32, sig: i32) -> SysResult<()> {
    let ret = unsafe { abi::syscall2(abi::SYS_KILL, pid as i64 as u64, sig as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn clock_gettime(clock_id: u64) -> SysResult<Timespec> {
    let mut ts = Timespec::default();
    let ret = unsafe {
        abi::syscall2(
            abi::SYS_CLOCK_GETTIME,
            clock_id,
            &mut ts as *mut Timespec as u64,
        )
    };
    decode_ret(ret).map(|_| ts)
}

#[inline]
pub fn nanosleep(req: &Timespec, rem: Option<&mut Timespec>) -> SysResult<()> {
    let rem_ptr = rem.map(|r| r as *mut Timespec as u64).unwrap_or(0);
    let ret = unsafe { abi::syscall2(abi::SYS_NANOSLEEP, req as *const Timespec as u64, rem_ptr) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn getrandom(buf: &mut [u8], flags: u32) -> SysResult<usize> {
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_GETRANDOM,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            flags as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn futex_wait(
    addr: &core::sync::atomic::AtomicU32,
    expected: u32,
    timeout: Option<&Timespec>,
) -> SysResult<()> {
    let t_ptr = timeout.map(|t| t as *const Timespec as u64).unwrap_or(0);
    let ret = unsafe {
        abi::syscall6(
            abi::SYS_FUTEX,
            addr as *const core::sync::atomic::AtomicU32 as u64,
            (FUTEX_WAIT | FUTEX_PRIVATE_FLAG) as u64,
            expected as u64,
            t_ptr,
            0,
            0,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn futex_wake(addr: &core::sync::atomic::AtomicU32, count: u32) -> SysResult<usize> {
    let ret = unsafe {
        abi::syscall6(
            abi::SYS_FUTEX,
            addr as *const core::sync::atomic::AtomicU32 as u64,
            (FUTEX_WAKE | FUTEX_PRIVATE_FLAG) as u64,
            count as u64,
            0,
            0,
            0,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn pledge(promises_mask: u32) -> SysResult<()> {
    let ret = unsafe { abi::syscall1(abi::SYS_EXO_PLEDGE, promises_mask as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn process_list(out: &mut [ExoProcessInfo]) -> SysResult<usize> {
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_EXO_PROCESS_LIST,
            out.as_mut_ptr() as u64,
            out.len() as u64,
            core::mem::size_of::<ExoProcessInfo>() as u64,
        )
    };
    decode_ret(ret)
}
