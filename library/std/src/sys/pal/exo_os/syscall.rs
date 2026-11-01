//! Raw Exo-OS x86_64 syscalls (`0..=549`) and typed ABI wrappers for `std::sys::pal::exo_os`.

#![allow(dead_code)]

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSTAT: u64 = 6;
pub const SYS_POLL: u64 = 7;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_PREAD64: u64 = 17;
pub const SYS_PWRITE64: u64 = 18;
pub const SYS_READV: u64 = 19;
pub const SYS_WRITEV: u64 = 20;
pub const SYS_ACCESS: u64 = 21;
pub const SYS_PIPE: u64 = 22;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_MREMAP: u64 = 25;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_ACCEPT: u64 = 43;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
pub const SYS_SENDMSG: u64 = 46;
pub const SYS_RECVMSG: u64 = 47;
pub const SYS_SHUTDOWN: u64 = 48;
pub const SYS_BIND: u64 = 49;
pub const SYS_LISTEN: u64 = 50;
pub const SYS_GETSOCKNAME: u64 = 51;
pub const SYS_GETPEERNAME: u64 = 52;
pub const SYS_SOCKETPAIR: u64 = 53;
pub const SYS_SETSOCKOPT: u64 = 54;
pub const SYS_GETSOCKOPT: u64 = 55;
pub const SYS_CLONE: u64 = 56;
pub const SYS_FORK: u64 = 57;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAIT4: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_FLOCK: u64 = 73;
pub const SYS_FSYNC: u64 = 74;
pub const SYS_FDATASYNC: u64 = 75;
pub const SYS_TRUNCATE: u64 = 76;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_GETDENTS: u64 = 78;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_RENAME: u64 = 82;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_LINK: u64 = 86;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_SYMLINK: u64 = 88;
pub const SYS_READLINK: u64 = 89;
pub const SYS_CHMOD: u64 = 90;
pub const SYS_FCHMOD: u64 = 91;
pub const SYS_GETPPID: u64 = 110;
pub const SYS_GETTID: u64 = 186;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_CTL: u64 = 233;
pub const SYS_EPOLL_CREATE1: u64 = 291;
pub const SYS_DUP3: u64 = 292;
pub const SYS_PIPE2: u64 = 293;
pub const SYS_GETRANDOM: u64 = 318;

pub const SYS_EXO_CAP_CREATE: u64 = 320;
pub const SYS_EXO_CAP_DELEGATE: u64 = 321;
pub const SYS_EXO_CAP_REVOKE: u64 = 322;
pub const SYS_EXO_CAP_CHECK: u64 = 323;
pub const SYS_EXO_PLEDGE: u64 = 324;
pub const SYS_EXO_LOG: u64 = 350;

pub const SYS_EXOFS_PATH_RESOLVE: u64 = 500;
pub const SYS_EXOFS_OBJECT_OPEN: u64 = 501;
pub const SYS_EXOFS_OBJECT_READ: u64 = 502;
pub const SYS_EXOFS_OBJECT_WRITE: u64 = 503;
pub const SYS_EXOFS_OBJECT_CREATE: u64 = 504;
pub const SYS_EXOFS_OBJECT_DELETE: u64 = 505;
pub const SYS_EXOFS_OBJECT_STAT: u64 = 506;
pub const SYS_EXOFS_OBJECT_SET_META: u64 = 507;
pub const SYS_EXOFS_GET_CONTENT_HASH: u64 = 508;
pub const SYS_EXOFS_OPEN_BY_PATH: u64 = 519;
pub const SYS_EXOFS_READDIR: u64 = 520;
pub const SYS_EXO_RESURRECT: u64 = 523;
pub const SYS_EXOPHOENIX_TEST: u64 = 524;

pub const EXOFS_RIGHT_READ: u32 = 1 << 0;
pub const EXOFS_RIGHT_WRITE: u32 = 1 << 1;
pub const EXOFS_RIGHT_CREATE: u32 = 1 << 2;
pub const EXOFS_RIGHT_DELETE: u32 = 1 << 3;
pub const EXOFS_RIGHT_STAT: u32 = 1 << 4;
pub const EXOFS_RIGHT_SETMETA: u32 = 1 << 5;
pub const EXOFS_RIGHT_LIST: u32 = 1 << 6;
pub const EXOFS_RIGHT_EXEC: u32 = 1 << 7;
pub const EXOFS_RIGHT_ADMIN: u32 = 1 << 16;
pub const EXOFS_RIGHT_ALL: u32 = 0x0000_FFFF;
pub const EXOFS_RIGHT_READ_ONLY: u32 = EXOFS_RIGHT_READ | EXOFS_RIGHT_STAT | EXOFS_RIGHT_LIST;
pub const EXOFS_RIGHT_READ_WRITE: u32 = EXOFS_RIGHT_READ
    | EXOFS_RIGHT_WRITE
    | EXOFS_RIGHT_CREATE
    | EXOFS_RIGHT_DELETE
    | EXOFS_RIGHT_STAT
    | EXOFS_RIGHT_SETMETA
    | EXOFS_RIGHT_LIST;

pub const EXOFS_STAT_LEN: usize = 104;
pub const EXOFS_OPEN_RDONLY: u32 = 0;
pub const EXOFS_OPEN_WRONLY: u32 = 1;
pub const EXOFS_OPEN_RDWR: u32 = 2;
pub const EXOFS_OPEN_CREATE: u32 = 0x0040;
pub const EXOFS_OPEN_EXCL: u32 = 0x0080;
pub const EXOFS_OPEN_TRUNC: u32 = 0x0200;
pub const EXOFS_OPEN_APPEND: u32 = 0x0400;
pub const EXOFS_OPEN_DIRECTORY: u32 = 0x0001_0000;

pub const EXOFS_SEEK_SET: u32 = 0;
pub const EXOFS_SEEK_CUR: u32 = 1;
pub const EXOFS_SEEK_END: u32 = 2;

pub const EXOFS_MODE_FILE: u32 = 0o100644;
pub const EXOFS_MODE_DIR: u32 = 0o040755;
pub const EXOFS_MODE_SYMLINK: u32 = 0o120777;

pub const O_CLOEXEC: i32 = 0o2000000;
pub const WNOHANG: i32 = 1;

pub const CLOCK_REALTIME: u64 = 0;
pub const CLOCK_MONOTONIC: u64 = 1;

pub const FUTEX_WAIT: u32 = 0;
pub const FUTEX_WAKE: u32 = 1;
pub const FUTEX_PRIVATE_FLAG: u32 = 128;

pub const AF_UNIX: i32 = 1;
pub const AF_INET: i32 = 2;
pub const AF_INET6: i32 = 10;

pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_NONBLOCK: i32 = 0o4000;
pub const SOCK_CLOEXEC: i32 = 0o2000000;

#[inline(always)]
pub unsafe fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") nr,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            in("r8") a5,
            in("r9") a6,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[inline(always)]
pub unsafe fn syscall0(nr: u64) -> i64 {
    unsafe { syscall6(nr, 0, 0, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
    unsafe { syscall6(nr, a1, 0, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, a3, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, a3, a4, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall5(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    unsafe { syscall6(nr, a1, a2, a3, a4, a5, 0) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SysError(pub i32);

impl SysError {
    pub const EPERM: Self = Self(1);
    pub const ENOENT: Self = Self(2);
    pub const ESRCH: Self = Self(3);
    pub const EINTR: Self = Self(4);
    pub const EIO: Self = Self(5);
    pub const ENXIO: Self = Self(6);
    pub const E2BIG: Self = Self(7);
    pub const ENOEXEC: Self = Self(8);
    pub const EBADF: Self = Self(9);
    pub const ECHILD: Self = Self(10);
    pub const EAGAIN: Self = Self(11);
    pub const ENOMEM: Self = Self(12);
    pub const EACCES: Self = Self(13);
    pub const EFAULT: Self = Self(14);
    pub const EBUSY: Self = Self(16);
    pub const EEXIST: Self = Self(17);
    pub const EXDEV: Self = Self(18);
    pub const ENODEV: Self = Self(19);
    pub const ENOTDIR: Self = Self(20);
    pub const EISDIR: Self = Self(21);
    pub const EINVAL: Self = Self(22);
    pub const ENFILE: Self = Self(23);
    pub const EMFILE: Self = Self(24);
    pub const ENOTTY: Self = Self(25);
    pub const EFBIG: Self = Self(27);
    pub const ENOSPC: Self = Self(28);
    pub const ESPIPE: Self = Self(29);
    pub const EROFS: Self = Self(30);
    pub const EPIPE: Self = Self(32);
    pub const ERANGE: Self = Self(34);
    pub const ENAMETOOLONG: Self = Self(36);
    pub const ENOSYS: Self = Self(38);
    pub const ENOTEMPTY: Self = Self(39);
    pub const ELOOP: Self = Self(40);
    pub const ENOTSOCK: Self = Self(88);
    pub const EAFNOSUPPORT: Self = Self(97);
    pub const EADDRINUSE: Self = Self(98);
    pub const EADDRNOTAVAIL: Self = Self(99);
    pub const ENETUNREACH: Self = Self(101);
    pub const ECONNABORTED: Self = Self(103);
    pub const ECONNRESET: Self = Self(104);
    pub const ENOBUFS: Self = Self(105);
    pub const EISCONN: Self = Self(106);
    pub const ENOTCONN: Self = Self(107);
    pub const ETIMEDOUT: Self = Self(110);
    pub const ECONNREFUSED: Self = Self(111);
    pub const EINPROGRESS: Self = Self(115);

    #[inline(always)]
    pub const fn errno(self) -> i32 {
        self.0
    }
}

pub type SysResult<T = usize> = Result<T, SysError>;

#[inline(always)]
pub const fn decode_ret(ret: i64) -> SysResult<usize> {
    if ret < 0 && ret >= -4095 {
        Err(SysError((-ret) as i32))
    } else {
        Ok(ret as usize)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapTokenUser {
    pub raw: [u8; 48],
}

impl Default for CapTokenUser {
    fn default() -> Self {
        Self { raw: [0u8; 48] }
    }
}

impl CapTokenUser {
    #[inline]
    pub fn generation(&self) -> u32 {
        u32::from_le_bytes([self.raw[32], self.raw[33], self.raw[34], self.raw[35]])
    }

    #[inline]
    pub fn rights(&self) -> u32 {
        u32::from_le_bytes([self.raw[36], self.raw[37], self.raw[38], self.raw[39]])
    }

    #[inline]
    pub fn owner_pid(&self) -> u32 {
        u32::from_le_bytes([self.raw[40], self.raw[41], self.raw[42], self.raw[43]])
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExoFsFileStat {
    pub object_id: [u8; 32],
    pub content_hash: [u8; 32],
    pub size: u64,
    pub mode: u32,
    pub flags: u32,
    pub ctime_ns: u64,
    pub mtime_ns: u64,
    pub generation: u64,
}

const _: () = assert!(core::mem::size_of::<ExoFsFileStat>() == EXOFS_STAT_LEN);

impl Default for ExoFsFileStat {
    fn default() -> Self {
        Self {
            object_id: [0u8; 32],
            content_hash: [0u8; 32],
            size: 0,
            mode: 0,
            flags: 0,
            ctime_ns: 0,
            mtime_ns: 0,
            generation: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ExofsPathResolveResult {
    pub blob_id: [u8; 32],
    pub object_id: [u8; 32],
    pub object_kind: u8,
    pub _pad: [u8; 7],
    pub size_bytes: u64,
    pub epoch_id: u64,
    pub link_count: u32,
    pub flags: u32,
    pub _reserved: [u8; 8],
}

const _: () = assert!(core::mem::size_of::<ExofsPathResolveResult>() == 104);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExoDirEntry {
    pub inode: u64,
    pub file_type: u8,
    pub name_len: u8,
    pub name: [u8; 246],
}

impl Default for ExoDirEntry {
    fn default() -> Self {
        Self {
            inode: 0,
            file_type: 0,
            name_len: 0,
            name: [0u8; 246],
        }
    }
}

impl ExoDirEntry {
    #[inline]
    pub fn name_bytes(&self) -> &[u8] {
        let len = (self.name_len as usize).min(self.name.len());
        let actual_len = self.name[..len]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(len);
        &self.name[..actual_len]
    }

    #[inline]
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(self.name_bytes()).unwrap_or("")
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SockAddrUn {
    pub sun_family: u16,
    pub sun_path: [u8; 108],
}

impl Default for SockAddrUn {
    fn default() -> Self {
        Self {
            sun_family: AF_UNIX as u16,
            sun_path: [0u8; 108],
        }
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpollEventAbi {
    pub events: u32,
    pub data: u64,
}

const _: () = assert!(core::mem::size_of::<EpollEventAbi>() == 12);

#[inline]
pub fn read(fd: i32, buf: &mut [u8]) -> SysResult<usize> {
    let ret = unsafe {
        syscall3(
            SYS_READ,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn pread64(fd: i32, buf: &mut [u8], offset: u64) -> SysResult<usize> {
    let ret = unsafe {
        syscall4(
            SYS_PREAD64,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            offset,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn write(fd: i32, buf: &[u8]) -> SysResult<usize> {
    let ret = unsafe {
        syscall3(
            SYS_WRITE,
            fd as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn pwrite64(fd: i32, buf: &[u8], offset: u64) -> SysResult<usize> {
    let ret = unsafe {
        syscall4(
            SYS_PWRITE64,
            fd as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
            offset,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn close(fd: i32) -> SysResult<()> {
    let ret = unsafe { syscall1(SYS_CLOSE, fd as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn dup(oldfd: i32) -> SysResult<i32> {
    let ret = unsafe { syscall1(SYS_DUP, oldfd as u64) };
    decode_ret(ret).map(|fd| fd as i32)
}

#[inline]
pub fn dup2(oldfd: i32, newfd: i32) -> SysResult<i32> {
    let ret = unsafe { syscall2(SYS_DUP2, oldfd as u64, newfd as u64) };
    decode_ret(ret).map(|fd| fd as i32)
}

#[inline]
pub fn pipe2(fds: &mut [i32; 2], flags: i32) -> SysResult<()> {
    let ret = unsafe { syscall2(SYS_PIPE2, fds.as_mut_ptr() as u64, flags as u32 as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn getcwd(buf: &mut [u8]) -> SysResult<usize> {
    let ret = unsafe { syscall2(SYS_GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64) };
    decode_ret(ret)
}

#[inline]
pub fn chdir(path_cstr: &[u8]) -> SysResult<()> {
    let ret = unsafe { syscall1(SYS_CHDIR, path_cstr.as_ptr() as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_open(path_cstr: &[u8], flags: u32, mode: u32) -> SysResult<i32> {
    if path_cstr.is_empty() {
        return Err(SysError::ENOENT);
    }
    let cap_rights = if (flags & (EXOFS_OPEN_WRONLY | EXOFS_OPEN_RDWR | EXOFS_OPEN_CREATE)) != 0 {
        EXOFS_RIGHT_READ_WRITE
    } else {
        EXOFS_RIGHT_READ_ONLY
    };
    let ret = unsafe {
        syscall6(
            SYS_EXOFS_OPEN_BY_PATH,
            path_cstr.as_ptr() as u64,
            flags as u64,
            mode as u64,
            0,
            0,
            cap_rights as u64,
        )
    };
    if ret >= 0 {
        return Ok(ret as i32);
    }
    let ret2 = unsafe {
        syscall3(
            SYS_OPEN,
            path_cstr.as_ptr() as u64,
            flags as u64,
            mode as u64,
        )
    };
    decode_ret(ret2).map(|fd| fd as i32)
}

#[inline]
pub fn exofs_stat(path_cstr: &[u8]) -> SysResult<ExoFsFileStat> {
    if path_cstr.is_empty() {
        return Err(SysError::ENOENT);
    }
    let trimmed_len = path_cstr
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(path_cstr.len());
    let mut resolved = ExofsPathResolveResult::default();
    let ret = unsafe {
        syscall6(
            SYS_EXOFS_PATH_RESOLVE,
            path_cstr.as_ptr() as u64,
            trimmed_len as u64,
            0,
            &mut resolved as *mut ExofsPathResolveResult as u64,
            0,
            EXOFS_RIGHT_STAT as u64,
        )
    };
    decode_ret(ret).map(|_| ExoFsFileStat {
        object_id: resolved.object_id,
        content_hash: resolved.blob_id,
        size: resolved.size_bytes,
        mode: if resolved.object_kind == 2 {
            EXOFS_MODE_DIR
        } else {
            EXOFS_MODE_FILE
        },
        flags: resolved.flags,
        ctime_ns: 0,
        mtime_ns: 0,
        generation: resolved.epoch_id,
    })
}

#[inline]
pub fn exofs_fstat(fd: i32) -> SysResult<ExoFsFileStat> {
    let mut st = ExoFsFileStat::default();
    let ret = unsafe {
        syscall3(
            SYS_EXOFS_OBJECT_STAT,
            fd as u64,
            &mut st as *mut ExoFsFileStat as u64,
            EXOFS_STAT_LEN as u64,
        )
    };
    decode_ret(ret).map(|_| st)
}

#[inline]
pub fn exofs_lseek(fd: i32, offset: i64, whence: u32) -> SysResult<u64> {
    let ret = unsafe { syscall3(SYS_LSEEK, fd as u64, offset as u64, whence as u64) };
    decode_ret(ret).map(|pos| pos as u64)
}

#[inline]
pub fn exofs_mkdir(path_cstr: &[u8], mode: u32) -> SysResult<()> {
    let ret = unsafe { syscall2(SYS_MKDIR, path_cstr.as_ptr() as u64, mode as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_unlink(path_cstr: &[u8], is_dir: bool) -> SysResult<()> {
    let nr = if is_dir { SYS_RMDIR } else { SYS_UNLINK };
    let ret = unsafe { syscall1(nr, path_cstr.as_ptr() as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_rename(old_cstr: &[u8], new_cstr: &[u8]) -> SysResult<()> {
    let ret = unsafe {
        syscall2(
            SYS_RENAME,
            old_cstr.as_ptr() as u64,
            new_cstr.as_ptr() as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_link(old_cstr: &[u8], new_cstr: &[u8]) -> SysResult<()> {
    let ret = unsafe {
        syscall2(
            SYS_LINK,
            old_cstr.as_ptr() as u64,
            new_cstr.as_ptr() as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_symlink(target_cstr: &[u8], linkpath_cstr: &[u8]) -> SysResult<()> {
    let ret = unsafe {
        syscall2(
            SYS_SYMLINK,
            target_cstr.as_ptr() as u64,
            linkpath_cstr.as_ptr() as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_readlink(path_cstr: &[u8], buf: &mut [u8]) -> SysResult<usize> {
    let ret = unsafe {
        syscall3(
            SYS_READLINK,
            path_cstr.as_ptr() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn exofs_chmod(path_cstr: &[u8], mode: u32) -> SysResult<()> {
    let ret = unsafe { syscall2(SYS_CHMOD, path_cstr.as_ptr() as u64, mode as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_fchmod(fd: i32, mode: u32) -> SysResult<()> {
    let ret = unsafe { syscall2(SYS_FCHMOD, fd as u64, mode as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_readdir(fd: i32, entries: &mut [ExoDirEntry]) -> SysResult<usize> {
    let byte_len = core::mem::size_of_val(entries);
    let ret = unsafe {
        syscall6(
            SYS_EXOFS_READDIR,
            fd as u64,
            entries.as_mut_ptr() as u64,
            byte_len as u64,
            0,
            0,
            EXOFS_RIGHT_LIST as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn exofs_ftruncate(fd: i32, length: u64) -> SysResult<()> {
    let ret = unsafe { syscall2(SYS_FTRUNCATE, fd as u64, length) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_fsync(fd: i32) -> SysResult<()> {
    let ret = unsafe { syscall1(SYS_FSYNC, fd as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exofs_delegate(
    parent_cap: &CapTokenUser,
    new_rights: u32,
    target_pid: u32,
) -> SysResult<CapTokenUser> {
    let mut out = CapTokenUser::default();
    let ret = unsafe {
        syscall4(
            SYS_EXO_CAP_DELEGATE,
            parent_cap as *const CapTokenUser as u64,
            new_rights as u64,
            target_pid as u64,
            &mut out as *mut CapTokenUser as u64,
        )
    };
    decode_ret(ret).map(|_| out)
}

#[inline]
pub fn getpid() -> u32 {
    unsafe { syscall0(SYS_GETPID) as u32 }
}

#[inline]
pub fn getppid() -> u32 {
    unsafe { syscall0(SYS_GETPPID) as u32 }
}

#[inline]
pub fn gettid() -> u32 {
    unsafe { syscall0(SYS_GETTID) as u32 }
}

#[inline]
pub fn sched_yield() {
    let _ = unsafe { syscall0(SYS_SCHED_YIELD) };
}

#[inline]
pub fn exit_group(code: i32) -> ! {
    unsafe {
        let _ = syscall1(SYS_EXIT_GROUP, code as u64);
        let _ = syscall1(SYS_EXIT, code as u64);
    }
    loop {
        core::hint::spin_loop();
    }
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
        syscall5(
            SYS_CLONE,
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
pub unsafe fn execve_raw(
    path_cstr: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> SysResult<()> {
    if path_cstr.is_null() || argv.is_null() {
        return Err(SysError::EFAULT);
    }
    let ret = unsafe {
        syscall3(
            SYS_EXECVE,
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
        syscall4(
            SYS_WAIT4,
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
    let ret = unsafe { syscall2(SYS_KILL, pid as i64 as u64, sig as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn clock_gettime(clock_id: u64) -> SysResult<RawTimespec> {
    let mut ts = RawTimespec::default();
    let ret = unsafe {
        syscall2(
            SYS_CLOCK_GETTIME,
            clock_id,
            &mut ts as *mut RawTimespec as u64,
        )
    };
    decode_ret(ret).map(|_| ts)
}

#[inline]
pub fn nanosleep(req: &RawTimespec, rem: Option<&mut RawTimespec>) -> SysResult<()> {
    let rem_ptr = rem.map(|r| r as *mut RawTimespec as u64).unwrap_or(0);
    let ret = unsafe { syscall2(SYS_NANOSLEEP, req as *const RawTimespec as u64, rem_ptr) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn getrandom(buf: &mut [u8], flags: u32) -> SysResult<usize> {
    let ret = unsafe {
        syscall3(
            SYS_GETRANDOM,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            flags as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn futex_wait_raw(
    addr: *const u32,
    expected: u32,
    timeout: Option<&RawTimespec>,
) -> SysResult<()> {
    let t_ptr = timeout.map(|t| t as *const RawTimespec as u64).unwrap_or(0);
    let ret = unsafe {
        syscall6(
            SYS_FUTEX,
            addr as u64,
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
pub fn futex_wake_raw(addr: *const u32, count: u32) -> SysResult<usize> {
    let ret = unsafe {
        syscall6(
            SYS_FUTEX,
            addr as u64,
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
pub fn socket(domain: i32, ty: i32, protocol: i32) -> SysResult<i32> {
    let ret = unsafe { syscall3(SYS_SOCKET, domain as u64, ty as u64, protocol as u64) };
    decode_ret(ret).map(|fd| fd as i32)
}

#[inline]
pub fn socketpair(domain: i32, ty: i32, protocol: i32) -> SysResult<[i32; 2]> {
    let mut sv = [0i32; 2];
    let ret = unsafe {
        syscall4(
            SYS_SOCKETPAIR,
            domain as u64,
            ty as u64,
            protocol as u64,
            sv.as_mut_ptr() as u64,
        )
    };
    decode_ret(ret).map(|_| sv)
}

#[inline]
pub fn connect_unix(fd: i32, addr: &SockAddrUn) -> SysResult<()> {
    let ret = unsafe {
        syscall3(
            SYS_CONNECT,
            fd as u64,
            addr as *const SockAddrUn as u64,
            core::mem::size_of::<SockAddrUn>() as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn bind_unix(fd: i32, addr: &SockAddrUn) -> SysResult<()> {
    let ret = unsafe {
        syscall3(
            SYS_BIND,
            fd as u64,
            addr as *const SockAddrUn as u64,
            core::mem::size_of::<SockAddrUn>() as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn accept_unix(fd: i32) -> SysResult<i32> {
    let ret = unsafe { syscall3(SYS_ACCEPT, fd as u64, 0, 0) };
    decode_ret(ret).map(|cfd| cfd as i32)
}

#[inline]
pub fn fcntl(fd: i32, cmd: i32, arg: u64) -> SysResult<i32> {
    let ret = unsafe { syscall3(SYS_FCNTL, fd as u64, cmd as u64, arg) };
    decode_ret(ret).map(|v| v as i32)
}

#[inline]
pub fn connect(fd: i32, addr: &SockAddrIn) -> SysResult<()> {
    let ret = unsafe {
        syscall3(
            SYS_CONNECT,
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
        syscall3(
            SYS_BIND,
            fd as u64,
            addr as *const SockAddrIn as u64,
            core::mem::size_of::<SockAddrIn>() as u64,
        )
    };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn listen(fd: i32, backlog: i32) -> SysResult<()> {
    let ret = unsafe { syscall2(SYS_LISTEN, fd as u64, backlog as u64) };
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
    let ret = unsafe { syscall3(SYS_ACCEPT, fd as u64, addr_ptr, len_ptr) };
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
        syscall6(
            SYS_SENDTO,
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
        syscall6(
            SYS_RECVFROM,
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
    let ret = unsafe { syscall2(SYS_SHUTDOWN, fd as u64, how as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn epoll_create1(flags: u32) -> SysResult<i32> {
    let ret = unsafe { syscall1(SYS_EPOLL_CREATE1, flags as u64) };
    decode_ret(ret).map(|fd| fd as i32)
}

#[inline]
pub fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: Option<&EpollEventAbi>) -> SysResult<()> {
    let ev_ptr = event.map(|e| e as *const EpollEventAbi as u64).unwrap_or(0);
    let ret = unsafe { syscall4(SYS_EPOLL_CTL, epfd as u64, op as u64, fd as u64, ev_ptr) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn epoll_wait(
    epfd: i32,
    events: &mut [EpollEventAbi],
    timeout_ms: i32,
) -> SysResult<usize> {
    let ret = unsafe {
        syscall4(
            SYS_EPOLL_WAIT,
            epfd as u64,
            events.as_mut_ptr() as u64,
            events.len() as u64,
            timeout_ms as i64 as u64,
        )
    };
    decode_ret(ret)
}

#[inline]
pub fn trigger_resurrection(mode: u64) -> SysResult<usize> {
    let ret = unsafe { syscall1(SYS_EXO_RESURRECT, mode) };
    decode_ret(ret)
}

#[inline]
pub fn exo_pledge(promises_mask: u32) -> SysResult<()> {
    let ret = unsafe { syscall1(SYS_EXO_PLEDGE, promises_mask as u64) };
    decode_ret(ret).map(|_| ())
}

#[inline]
pub fn exo_log(msg: &[u8]) {
    let _ = unsafe { syscall2(SYS_EXO_LOG, msg.as_ptr() as u64, msg.len() as u64) };
}
