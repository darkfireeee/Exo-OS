//! Primitives réseau et multiplexage (`socket`, `connect`, `bind`, `listen`, `accept`, `epoll`).

use crate::{abi, decode_ret, EpollEvent, SysResult};

pub const AF_UNIX: i32 = 1;
pub const AF_INET: i32 = 2;
pub const AF_INET6: i32 = 10;

pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_NONBLOCK: i32 = 0o4000;
pub const SOCK_CLOEXEC: i32 = 0o2000000;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

#[inline]
pub fn socket(domain: i32, ty: i32, protocol: i32) -> SysResult<i32> {
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_SOCKET,
            domain as u64,
            ty as u64,
            protocol as u64,
        )
    };
    decode_ret(ret).map(|fd| fd as i32)
}

#[inline]
pub fn connect(fd: i32, addr: &SockAddrIn) -> SysResult<()> {
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_CONNECT,
            fd as u64,
            addr as *const SockAddrIn as u64,
            core::mem::size_of::<SockAddrIn>() as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn bind(fd: i32, addr: &SockAddrIn) -> SysResult<()> {
    let ret = unsafe {
        abi::syscall3(
            abi::SYS_BIND,
            fd as u64,
            addr as *const SockAddrIn as u64,
            core::mem::size_of::<SockAddrIn>() as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn listen(fd: i32, backlog: i32) -> SysResult<()> {
    let ret = unsafe { abi::syscall2(abi::SYS_LISTEN, fd as u64, backlog as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn accept(fd: i32, addr_out: Option<&mut SockAddrIn>) -> SysResult<i32> {
    let mut addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    let (addr_ptr, len_ptr) = match addr_out {
        Some(a) => (
            a as *mut SockAddrIn as u64,
            &mut addrlen as *mut u32 as u64,
        ),
        None => (0, 0),
    };
    let ret = unsafe { abi::syscall3(abi::SYS_ACCEPT, fd as u64, addr_ptr, len_ptr) };
    decode_ret(ret).map(|cfd| cfd as i32)
}

#[inline]
pub fn sendto(fd: i32, buf: &[u8], flags: u32, dest: Option<&SockAddrIn>) -> SysResult<usize> {
    let (addr_ptr, addr_len) = match dest {
        Some(a) => (
            a as *const SockAddrIn as u64,
            core::mem::size_of::<SockAddrIn>() as u64,
        ),
        None => (0, 0),
    };
    let ret = unsafe {
        abi::syscall6(
            abi::SYS_SENDTO,
            fd as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
            flags as u64,
            addr_ptr,
            addr_len,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn recvfrom(
    fd: i32,
    buf: &mut [u8],
    flags: u32,
    src_out: Option<&mut SockAddrIn>,
) -> SysResult<usize> {
    let mut addrlen = core::mem::size_of::<SockAddrIn>() as u32;
    let (addr_ptr, len_ptr) = match src_out {
        Some(a) => (
            a as *mut SockAddrIn as u64,
            &mut addrlen as *mut u32 as u64,
        ),
        None => (0, 0),
    };
    let ret = unsafe {
        abi::syscall6(
            abi::SYS_RECVFROM,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            flags as u64,
            addr_ptr,
            len_ptr,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn shutdown(fd: i32, how: i32) -> SysResult<()> {
    let ret = unsafe { abi::syscall2(abi::SYS_SHUTDOWN, fd as u64, how as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn epoll_create1(flags: u32) -> SysResult<i32> {
    let ret = unsafe { abi::syscall1(abi::SYS_EPOLL_CREATE1, flags as u64) };
    decode_ret(ret).map(|fd| fd as i32)
}

#[inline]
pub fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: Option<&EpollEvent>) -> SysResult<()> {
    let ev_ptr = event.map(|e| e as *const EpollEvent as u64).unwrap_or(0);
    let ret = unsafe {
        abi::syscall4(
            abi::SYS_EPOLL_CTL,
            epfd as u64,
            op as u64,
            fd as u64,
            ev_ptr,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn epoll_wait(epfd: i32, events: &mut [EpollEvent], timeout_ms: i32) -> SysResult<usize> {
    let ret = unsafe {
        abi::syscall4(
            abi::SYS_EPOLL_WAIT,
            epfd as u64,
            events.as_mut_ptr() as u64,
            events.len() as u64,
            timeout_ms as i64 as u64,
        )
    };
    decode_ret(ret)
}
