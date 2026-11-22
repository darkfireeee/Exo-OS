//! Minimal raw x86_64 Exo-OS syscall wrappers for low-level compliance probes.
#![allow(dead_code)]

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_POLL: u64 = 7;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_IOCTL: u64 = 16;
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
pub const SYS_KILL: u64 = 62;
pub const SYS_UNAME: u64 = 63;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_FLOCK: u64 = 73;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_SYMLINK: u64 = 88;
pub const SYS_REBOOT: u64 = 169;
pub const SYS_GETTID: u64 = 186;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_CTL: u64 = 233;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_UNLINKAT: u64 = 263;
pub const SYS_UTIMENSAT: u64 = 280;
pub const SYS_EVENTFD2: u64 = 290;
pub const SYS_EPOLL_CREATE1: u64 = 291;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_MEMFD_CREATE: u64 = 319;
pub const SYS_EXO_IPC_LOOKUP: u64 = 306;
pub const SYS_EXO_PLEDGE: u64 = 324;
pub const SYS_EXO_LOG: u64 = 350;
pub const SYS_PHOENIX_FORCE: u64 = 523;
pub const SYS_EXOPHOENIX_TEST: u64 = 524;

pub const PROT_EXEC: u64 = 4;
pub const MAP_SHARED: u64 = 0x01;
pub const MAP_FIXED: u64 = 0x10;

pub const MFD_ALLOW_SEALING: u64 = 0x0002;
pub const F_ADD_SEALS: u64 = 1033;
pub const F_GET_SEALS: u64 = 1034;
pub const F_SEAL_SHRINK: u64 = 0x0002;

pub const EPERM: i64 = -1;
pub const EACCES: i64 = -13;
pub const EMFILE: i64 = -24;
pub const ENOSPC: i64 = -28;

pub const PHOENIX_METRICS_MAGIC: u32 = 0x504D_4554;

#[repr(C, align(64))]
#[derive(Clone, Copy, Default)]
pub struct PhoenixTimingRecordWire {
    pub magic: u32,
    pub cycle_id: u32,
    pub trigger_reason: u32,
    pub flags: u32,
    pub tsc_khz: u64,
    pub t0_freeze_req: u64,
    pub t1_freeze_ack_all: u64,
    pub t2_iommu_and_forge: u64,
    pub t3_kernel_b_active: u64,
    pub t4_crypto_reseeded: u64,
    pub t5_strate1_ready: u64,
    pub t6_strate2_ready: u64,
    pub delta_s0_freeze_us: u64,
    pub delta_s0_forge_us: u64,
    pub delta_s0_us: u64,
    pub delta_crypto_us: u64,
    pub delta_sla_us: u64,
    pub delta_full_us: u64,
    pub crypto_epoch_id: u64,
}

pub const LINUX_REBOOT_MAGIC1: u64 = 0xfee1_dead;
pub const LINUX_REBOOT_MAGIC2: u64 = 672_274_793;
pub const EXO_REBOOT_CMD_COMPLIANCE_EXIT: u64 = 0x4558_4f54;
pub const QEMU_DEBUG_EXIT_SUCCESS: u64 = 0x10;
pub const QEMU_DEBUG_EXIT_FAILURE: u64 = 0x11;

pub const AF_UNIX: i32 = 1;
pub const AF_INET: i32 = 2;
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_NONBLOCK: i32 = 0o4000;
pub const SOCK_CLOEXEC: i32 = 0o2000000;

pub const SOL_SOCKET: i32 = 1;
pub const SCM_RIGHTS: i32 = 1;
pub const SO_ERROR: i32 = 4;
pub const SO_PEERCRED: i32 = 17;

pub const MSG_PEEK: u32 = 0x02;
pub const MSG_CTRUNC: i32 = 0x08;
pub const MSG_DONTWAIT: u32 = 0x40;
pub const MSG_NOSIGNAL: u32 = 0x4000;

pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;
pub const EPOLLIN: u32 = 0x0001;
pub const EPOLLOUT: u32 = 0x0004;
pub const EPOLLERR: u32 = 0x0008;
pub const EPOLLHUP: u32 = 0x0010;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLONESHOT: u32 = 1 << 30;
pub const EPOLLET: u32 = 1 << 31;

pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_ANONYMOUS: u64 = 0x20;

pub const SIGUSR1: u64 = 10;
pub const SIG_BLOCK: u64 = 0;
pub const SIG_UNBLOCK: u64 = 1;
pub const SIG_SETMASK: u64 = 2;
pub const SA_RESTORER: u64 = 0x0400_0000;

pub const FUTEX_WAIT_BITSET: u32 = 9;
pub const FUTEX_WAKE_BITSET: u32 = 10;
pub const FUTEX_CMP_REQUEUE: u32 = 4;
pub const FUTEX_PRIVATE_FLAG: u32 = 128;

pub const F_GETFL: u64 = 3;
pub const F_SETFL: u64 = 4;
pub const O_RDONLY: u64 = 0;
pub const O_RDWR: u64 = 2;
pub const O_CREAT: u64 = 0o100;
pub const O_TRUNC: u64 = 0o1000;
pub const O_NONBLOCK: u64 = 0o4000;

pub const LOCK_SH: u64 = 1;
pub const LOCK_EX: u64 = 2;
pub const LOCK_NB: u64 = 4;
pub const LOCK_UN: u64 = 8;

pub const AT_FDCWD: i32 = -100;

pub const EAGAIN: i64 = -11;
pub const ENOTDIR: i64 = -20;
pub const ENOTTY: i64 = -25;
pub const ELOOP: i64 = -40;
pub const ETIMEDOUT: i64 = -110;
pub const ECONNREFUSED: i64 = -111;
pub const EINPROGRESS: i64 = -115;

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

const _: () = assert!(core::mem::size_of::<EpollEvent>() == 12);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Ucred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Iovec {
    pub iov_base: u64,
    pub iov_len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Msghdr {
    pub msg_name: u64,
    pub msg_namelen: u32,
    pub _pad0: u32,
    pub msg_iov: u64,
    pub msg_iovlen: u64,
    pub msg_control: u64,
    pub msg_controllen: u64,
    pub msg_flags: i32,
    pub _pad1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LinuxStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub _pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    pub _reserved: [i64; 3],
}

impl Default for LinuxStat {
    fn default() -> Self {
        unsafe { core::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KernelSigAction {
    pub sa_handler: u64,
    pub sa_flags: u64,
    pub sa_restorer: u64,
    pub sa_mask: u64,
}

#[repr(C)]
pub struct UtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

impl Default for UtsName {
    fn default() -> Self {
        unsafe { core::mem::zeroed() }
    }
}

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

pub fn exo_log_line(line: &str) {
    let mut bytes = line.as_bytes().to_vec();
    bytes.push(b'\n');
    unsafe {
        let _ = syscall2(SYS_EXO_LOG, bytes.as_ptr() as u64, bytes.len() as u64);
    }
}

pub fn compliance_exit_qemu(all_pass: bool) -> ! {
    let code = if all_pass {
        QEMU_DEBUG_EXIT_SUCCESS
    } else {
        QEMU_DEBUG_EXIT_FAILURE
    };
    unsafe {
        let _ = syscall4(
            SYS_REBOOT,
            LINUX_REBOOT_MAGIC1,
            LINUX_REBOOT_MAGIC2,
            EXO_REBOOT_CMD_COMPLIANCE_EXIT,
            code,
        );
    }
    std::process::exit(if all_pass { 0 } else { 1 });
}

#[unsafe(naked)]
pub unsafe extern "C" fn sigreturn_trampoline() {
    core::arch::naked_asm!(
        "mov rax, 15",
        "syscall",
        "ud2",
    );
}
