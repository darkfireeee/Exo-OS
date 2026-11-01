//! Traits de la Platform Abstraction Layer (`Pal`, `PalSignal`, `PalThread`, `PalSocket`)
//! inspirés de `relibc` et adaptés aux invariants d'Exo-OS.

pub mod exo_os;

use exo_rt::SigActionSlot;
use exo_syscall::exofs::{ExoDirEntry, ExoFsFileStat};
use exo_syscall::net::SockAddrIn;
use exo_syscall::process::Timespec;
use exo_syscall::{EpollEvent, SysResult};

pub trait Pal {
    fn open(path_cstr: &[u8], posix_flags: i32, mode: u32) -> SysResult<i32>;
    fn read(fd: i32, buf: &mut [u8]) -> SysResult<usize>;
    fn write(fd: i32, buf: &[u8]) -> SysResult<usize>;
    fn close(fd: i32) -> SysResult<()>;
    fn stat(path_cstr: &[u8]) -> SysResult<ExoFsFileStat>;
    fn fstat(fd: i32) -> SysResult<ExoFsFileStat>;
    fn lseek(fd: i32, offset: i64, whence: i32) -> SysResult<u64>;
    fn mkdir(path_cstr: &[u8], mode: u32) -> SysResult<()>;
    fn unlink(path_cstr: &[u8], flags: u32) -> SysResult<()>;
    fn rename(old_cstr: &[u8], new_cstr: &[u8]) -> SysResult<()>;
    fn getdents(fd: i32, entries: &mut [ExoDirEntry]) -> SysResult<usize>;
    fn ftruncate(fd: i32, len: u64) -> SysResult<()>;
    fn fsync(fd: i32) -> SysResult<()>;

    fn getpid() -> u32;
    fn getppid() -> u32;
    fn fork() -> SysResult<u32>;
    unsafe fn execve(
        path_cstr: *const u8,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> SysResult<()>;
    fn wait4(pid: i32, status: &mut i32, options: i32) -> SysResult<u32>;
    fn exit(code: i32) -> !;
    fn clock_gettime(clock_id: u64) -> SysResult<Timespec>;
    fn nanosleep(req: &Timespec, rem: Option<&mut Timespec>) -> SysResult<()>;
    fn getrandom(buf: &mut [u8], flags: u32) -> SysResult<usize>;
}

pub trait PalSignal {
    fn sigaction(
        sig: u8,
        new_act: Option<SigActionSlot>,
        old_act: Option<&mut SigActionSlot>,
    ) -> SysResult<()>;
    fn sigprocmask(how: i32, set: Option<u64>, oldset: Option<&mut u64>) -> SysResult<()>;
    fn kill(pid: i32, sig: i32) -> SysResult<()>;
}

pub trait PalThread {
    fn gettid() -> u32;
    fn sched_yield();
    fn futex_wait(
        addr: &core::sync::atomic::AtomicU32,
        expected: u32,
        timeout: Option<&Timespec>,
    ) -> SysResult<()>;
    fn futex_wake(addr: &core::sync::atomic::AtomicU32, count: u32) -> SysResult<usize>;
}

pub trait PalSocket {
    fn socket(domain: i32, ty: i32, protocol: i32) -> SysResult<i32>;
    fn connect(fd: i32, addr: &SockAddrIn) -> SysResult<()>;
    fn bind(fd: i32, addr: &SockAddrIn) -> SysResult<()>;
    fn listen(fd: i32, backlog: i32) -> SysResult<()>;
    fn accept(fd: i32, addr_out: Option<&mut SockAddrIn>) -> SysResult<i32>;
    fn sendto(fd: i32, buf: &[u8], flags: u32, dest: Option<&SockAddrIn>) -> SysResult<usize>;
    fn recvfrom(
        fd: i32,
        buf: &mut [u8],
        flags: u32,
        src_out: Option<&mut SockAddrIn>,
    ) -> SysResult<usize>;
    fn shutdown(fd: i32, how: i32) -> SysResult<()>;
    fn epoll_create1(flags: u32) -> SysResult<i32>;
    fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: Option<&EpollEvent>) -> SysResult<()>;
    fn epoll_wait(epfd: i32, events: &mut [EpollEvent], timeout_ms: i32) -> SysResult<usize>;
}
