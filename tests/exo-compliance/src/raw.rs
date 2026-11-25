//! Minimal raw x86_64 Exo-OS syscall wrappers for low-level compliance probes.
#![allow(dead_code)]

pub use exo_abi::errno::{
    EACCES, EAGAIN, ECONNREFUSED, EINPROGRESS, ELOOP, EMFILE, ENOSPC, ENOTDIR, ENOTTY, EPERM,
    ETIMEDOUT,
};
pub use exo_abi::numbers::{
    SYS_ACCEPT, SYS_BIND, SYS_CLOSE, SYS_CONNECT, SYS_EPOLL_CREATE1, SYS_EPOLL_CTL,
    SYS_EPOLL_WAIT, SYS_EVENTFD2, SYS_EXECVE, SYS_EXOPHOENIX_TEST, SYS_EXO_CAP_CREATE,
    SYS_EXO_LOG, SYS_EXO_PLEDGE, SYS_EXO_PROCESS_LIST, SYS_FCNTL, SYS_FLOCK, SYS_FTRUNCATE,
    SYS_FUTEX, SYS_GETPEERNAME, SYS_GETPID, SYS_GETRANDOM, SYS_GETSOCKNAME, SYS_GETSOCKOPT,
    SYS_GETTID, SYS_IOCTL, SYS_IPC_LOOKUP as SYS_EXO_IPC_LOOKUP, SYS_IPC_RECV as SYS_EXO_IPC_RECV,
    SYS_IPC_SEND as SYS_EXO_IPC_SEND, SYS_KILL, SYS_LISTEN, SYS_LSEEK, SYS_MEMFD_CREATE,
    SYS_MMAP, SYS_MPROTECT, SYS_MUNMAP, SYS_NEWFSTATAT, SYS_OPEN, SYS_OPENAT, SYS_PHOENIX_FORCE,
    SYS_POLL, SYS_READ, SYS_REBOOT, SYS_RECVFROM, SYS_RECVMSG, SYS_RT_SIGACTION,
    SYS_RT_SIGPROCMASK, SYS_RT_SIGRETURN, SYS_SENDMSG, SYS_SENDTO, SYS_SETSOCKOPT, SYS_SHUTDOWN,
    SYS_SOCKET, SYS_SOCKETPAIR, SYS_STAT, SYS_SYMLINK, SYS_UNAME, SYS_UNLINKAT, SYS_UTIMENSAT,
    SYS_WRITE,
};
pub use exo_abi::{
    syscall0, syscall1, syscall2, syscall3, syscall4, syscall5, syscall6,
    ExoProcessInfo as ExoProcessInfoWire,
};

pub const PHOENIX_FLAG_DEGRADED_FREEZE_TIMEOUT: u32 = 1 << 0;
pub const PHOENIX_FLAG_COLD_DISK_FALLBACK: u32 = 1 << 1;
pub const PHOENIX_FLAG_TEXT_HEALED: u32 = 1 << 2;
pub const PHOENIX_FLAG_COLD_RESPAWN: u32 = 1 << 3;
pub const PHOENIX_FLAG_CRASH_NMI: u32 = 1 << 4;
pub const PHOENIX_FLAG_KVM_ACCEL: u32 = 1 << 5;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PhoenixLeakSnapshotWire {
    pub free_frames: u64,
    pub active_pids: u64,
    pub p0_used: u64,
    pub ledger_verified: u64,
    pub online_cpus: u64,
    pub is_kvm: u64,
    pub vtd_units: u64,
    pub heartbeat: u64,
}

pub const PROT_EXEC: u64 = 4;
pub const MAP_SHARED: u64 = 0x01;
pub const MAP_FIXED: u64 = 0x10;

pub const MFD_ALLOW_SEALING: u64 = 0x0002;
pub const F_ADD_SEALS: u64 = 1033;
pub const F_GET_SEALS: u64 = 1034;
pub const F_SEAL_SHRINK: u64 = 0x0002;

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
