//! Implémentation de `Pal`, `PalSignal`, `PalThread`, `PalSocket` pour `ExoOs`.
//!
//! Respecte :
//! - `LIB-09` : Adaptation locale intégrée dans `libs/exo-libc` sans dépendance externe `relibc`.
//! - `PROC-04` : `fflush_all()` appelé avant `exo_rt::fork()`.
//! - `SIG-01..SIG-24` : `sigaction` et `sigprocmask` 0-syscall via `SignalTcb`.
//! - `ExoFS 500..=520` : traduction des flags POSIX vers les primitives ExoFS natives.

use crate::fflush_all;
use crate::platform::{Pal, PalSignal, PalSocket, PalThread};
use exo_phoenix_ssr::{PhoenixError, PhoenixRecovery, PhoenixSafe};
use exo_rt::SigActionSlot;
use exo_syscall::exofs::{
    self, ExoDirEntry, ExoFsFileStat, EXOFS_OPEN_APPEND, EXOFS_OPEN_CREATE, EXOFS_OPEN_DIRECTORY,
    EXOFS_OPEN_EXCL, EXOFS_OPEN_RDONLY, EXOFS_OPEN_RDWR, EXOFS_OPEN_TRUNC, EXOFS_OPEN_WRONLY,
    EXOFS_SEEK_CUR, EXOFS_SEEK_END, EXOFS_SEEK_SET,
};
use exo_syscall::net::{self, SockAddrIn};
use exo_syscall::process::{self, Timespec};
use exo_syscall::{EpollEvent, SysError, SysResult};

pub const O_RDONLY: i32 = 0;
pub const O_WRONLY: i32 = 1;
pub const O_RDWR: i32 = 2;
pub const O_CREAT: i32 = 0o100;
pub const O_EXCL: i32 = 0o200;
pub const O_TRUNC: i32 = 0o1000;
pub const O_APPEND: i32 = 0o2000;
pub const O_DIRECTORY: i32 = 0o200000;

/// Traduit les flags d'ouverture POSIX en flags natifs `EXOFS_OPEN_*`.
#[inline]
pub const fn posix_flags_to_exofs(posix: i32) -> u32 {
    let mut out = match posix & 0b11 {
        O_WRONLY => EXOFS_OPEN_WRONLY,
        O_RDWR => EXOFS_OPEN_RDWR,
        _ => EXOFS_OPEN_RDONLY,
    };
    if (posix & O_CREAT) != 0 {
        out |= EXOFS_OPEN_CREATE;
    }
    if (posix & O_EXCL) != 0 {
        out |= EXOFS_OPEN_EXCL;
    }
    if (posix & O_TRUNC) != 0 {
        out |= EXOFS_OPEN_TRUNC;
    }
    if (posix & O_APPEND) != 0 {
        out |= EXOFS_OPEN_APPEND;
    }
    if (posix & O_DIRECTORY) != 0 {
        out |= EXOFS_OPEN_DIRECTORY;
    }
    out
}

pub struct ExoOs;

impl Pal for ExoOs {
    #[inline]
    fn open(path_cstr: &[u8], posix_flags: i32, mode: u32) -> SysResult<i32> {
        let exofs_flags = posix_flags_to_exofs(posix_flags);
        exofs::exofs_open(path_cstr, exofs_flags, mode)
    }

    #[inline]
    fn read(fd: i32, buf: &mut [u8]) -> SysResult<usize> {
        // Les FDs stdio (0, 1, 2), pipes/PTYs ou sockets (>= 0x4000_0000) passent par SYS_READ (0),
        // qui route automatiquement vers TTY, pipe, socket ou ExoFS dans le noyau.
        if fd <= 2 || (fd as u32 & 0x4000_0000) != 0 {
            exofs::read(fd, buf)
        } else {
            exofs::exofs_read(fd, buf).or_else(|_| exofs::read(fd, buf))
        }
    }

    #[inline]
    fn write(fd: i32, buf: &[u8]) -> SysResult<usize> {
        if fd <= 2 || (fd as u32 & 0x4000_0000) != 0 {
            exofs::write(fd, buf)
        } else {
            exofs::exofs_write(fd, buf).or_else(|_| exofs::write(fd, buf))
        }
    }

    #[inline]
    fn close(fd: i32) -> SysResult<()> {
        if fd <= 2 || (fd as u32 & 0x4000_0000) != 0 {
            exofs::close(fd)
        } else {
            exofs::exofs_close(fd).or_else(|_| exofs::close(fd))
        }
    }

    #[inline]
    fn stat(path_cstr: &[u8]) -> SysResult<ExoFsFileStat> {
        exofs::exofs_stat(path_cstr)
    }

    #[inline]
    fn fstat(fd: i32) -> SysResult<ExoFsFileStat> {
        exofs::exofs_fstat(fd)
    }

    #[inline]
    fn lseek(fd: i32, offset: i64, whence: i32) -> SysResult<u64> {
        let w = match whence {
            0 => EXOFS_SEEK_SET,
            1 => EXOFS_SEEK_CUR,
            2 => EXOFS_SEEK_END,
            _ => return Err(SysError::EINVAL),
        };
        exofs::exofs_lseek(fd, offset, w)
    }

    #[inline]
    fn mkdir(path_cstr: &[u8], mode: u32) -> SysResult<()> {
        exofs::exofs_mkdir(path_cstr, mode)
    }

    #[inline]
    fn unlink(path_cstr: &[u8], flags: u32) -> SysResult<()> {
        exofs::exofs_unlink(path_cstr, flags)
    }

    #[inline]
    fn rename(old_cstr: &[u8], new_cstr: &[u8]) -> SysResult<()> {
        exofs::exofs_rename(old_cstr, new_cstr)
    }

    #[inline]
    fn getdents(fd: i32, entries: &mut [ExoDirEntry]) -> SysResult<usize> {
        exofs::exofs_readdir(fd, entries)
    }

    #[inline]
    fn ftruncate(fd: i32, len: u64) -> SysResult<()> {
        exofs::exofs_ftruncate(fd, len)
    }

    #[inline]
    fn fsync(fd: i32) -> SysResult<()> {
        exofs::exofs_fsync(fd)
    }

    #[inline]
    fn getpid() -> u32 {
        process::getpid()
    }

    #[inline]
    fn getppid() -> u32 {
        process::getppid()
    }

    #[inline]
    fn fork() -> SysResult<u32> {
        // RÈGLE PROC-04 : vidage intégral des tampons stdio avant fork() pour éviter la duplication d'I/O.
        fflush_all();
        exo_rt::fork().map_err(SysError)
    }

    #[inline]
    unsafe fn execve(
        path_cstr: *const u8,
        argv: *const *const u8,
        envp: *const *const u8,
    ) -> SysResult<()> {
        unsafe { exo_rt::execve(path_cstr, argv, envp).map_err(SysError) }
    }

    #[inline]
    fn wait4(pid: i32, status: &mut i32, options: i32) -> SysResult<u32> {
        process::wait4(pid, status, options)
    }

    #[inline]
    fn exit(code: i32) -> ! {
        fflush_all();
        process::exit_group(code)
    }

    #[inline]
    fn clock_gettime(clock_id: u64) -> SysResult<Timespec> {
        process::clock_gettime(clock_id)
    }

    #[inline]
    fn nanosleep(req: &Timespec, rem: Option<&mut Timespec>) -> SysResult<()> {
        process::nanosleep(req, rem)
    }

    #[inline]
    fn getrandom(buf: &mut [u8], flags: u32) -> SysResult<usize> {
        process::getrandom(buf, flags)
    }
}

impl PalSignal for ExoOs {
    #[inline]
    fn sigaction(
        sig: u8,
        new_act: Option<SigActionSlot>,
        old_act: Option<&mut SigActionSlot>,
    ) -> SysResult<()> {
        exo_rt::sigaction_0syscall(sig, new_act, old_act).map_err(SysError)
    }

    #[inline]
    fn sigprocmask(how: i32, set: Option<u64>, oldset: Option<&mut u64>) -> SysResult<()> {
        exo_rt::sigprocmask_0syscall(how, set, oldset).map_err(SysError)
    }

    #[inline]
    fn kill(pid: i32, sig: i32) -> SysResult<()> {
        process::kill(pid, sig)
    }
}

impl PalThread for ExoOs {
    #[inline]
    fn gettid() -> u32 {
        process::gettid()
    }

    #[inline]
    fn sched_yield() {
        process::sched_yield();
    }

    #[inline]
    fn futex_wait(
        addr: &core::sync::atomic::AtomicU32,
        expected: u32,
        timeout: Option<&Timespec>,
    ) -> SysResult<()> {
        process::futex_wait(addr, expected, timeout)
    }

    #[inline]
    fn futex_wake(addr: &core::sync::atomic::AtomicU32, count: u32) -> SysResult<usize> {
        process::futex_wake(addr, count)
    }
}

impl PalSocket for ExoOs {
    #[inline]
    fn socket(domain: i32, ty: i32, protocol: i32) -> SysResult<i32> {
        net::socket(domain, ty, protocol)
    }

    #[inline]
    fn connect(fd: i32, addr: &SockAddrIn) -> SysResult<()> {
        net::connect(fd, addr)
    }

    #[inline]
    fn bind(fd: i32, addr: &SockAddrIn) -> SysResult<()> {
        net::bind(fd, addr)
    }

    #[inline]
    fn listen(fd: i32, backlog: i32) -> SysResult<()> {
        net::listen(fd, backlog)
    }

    #[inline]
    fn accept(fd: i32, addr_out: Option<&mut SockAddrIn>) -> SysResult<i32> {
        net::accept(fd, addr_out)
    }

    #[inline]
    fn sendto(fd: i32, buf: &[u8], flags: u32, dest: Option<&SockAddrIn>) -> SysResult<usize> {
        net::sendto(fd, buf, flags, dest)
    }

    #[inline]
    fn recvfrom(
        fd: i32,
        buf: &mut [u8],
        flags: u32,
        src_out: Option<&mut SockAddrIn>,
    ) -> SysResult<usize> {
        net::recvfrom(fd, buf, flags, src_out)
    }

    #[inline]
    fn shutdown(fd: i32, how: i32) -> SysResult<()> {
        net::shutdown(fd, how)
    }

    #[inline]
    fn epoll_create1(flags: u32) -> SysResult<i32> {
        net::epoll_create1(flags)
    }

    #[inline]
    fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: Option<&EpollEvent>) -> SysResult<()> {
        net::epoll_ctl(epfd, op, fd, event)
    }

    #[inline]
    fn epoll_wait(epfd: i32, events: &mut [EpollEvent], timeout_ms: i32) -> SysResult<usize> {
        net::epoll_wait(epfd, events, timeout_ms)
    }
}

impl PhoenixSafe for ExoOs {
    const RECOVERY: PhoenixRecovery = PhoenixRecovery::Stateless;

    fn on_pre_switch(&self) -> Result<(), PhoenixError> {
        fflush_all();
        Ok(())
    }

    fn on_post_switch(&self) -> Result<(), PhoenixError> {
        Ok(())
    }
}
