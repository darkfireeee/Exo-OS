#![no_std]

#[inline(always)]
pub unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
    unsafe { syscall6(nr, a1, 0, 0, 0, 0, 0) }
}

#[inline(always)]
pub unsafe fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
    let ret: i64;
    // SAFETY: the caller is responsible for passing kernel-valid arguments.
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
#[deprecated(note = "alias compatibilite: utiliser SYS_RT_SIGACTION")]
pub const SYS_SIGACTION: u64 = SYS_RT_SIGACTION;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_PREAD64: u64 = 17;
pub const SYS_PWRITE64: u64 = 18;
pub const SYS_READV: u64 = 19;
pub const SYS_WRITEV: u64 = 20;
pub const SYS_ACCESS: u64 = 21;
pub const SYS_PIPE: u64 = 22;
pub const SYS_SELECT: u64 = 23;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_MREMAP: u64 = 25;
pub const SYS_MSYNC: u64 = 26;
pub const SYS_MINCORE: u64 = 27;
pub const SYS_MADVISE: u64 = 28;
pub const SYS_SHMGET: u64 = 29;
pub const SYS_SHMAT: u64 = 30;
pub const SYS_SHMCTL: u64 = 31;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_PAUSE: u64 = 34;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_GETITIMER: u64 = 36;
pub const SYS_ALARM: u64 = 37;
pub const SYS_SETITIMER: u64 = 38;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SENDFILE: u64 = 40;
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
pub const SYS_VFORK: u64 = 58;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAIT4: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_UNAME: u64 = 63;
pub const SYS_SEMGET: u64 = 64;
pub const SYS_SEMOP: u64 = 65;
pub const SYS_SEMCTL: u64 = 66;
pub const SYS_SHMDT: u64 = 67;
pub const SYS_MSGGET: u64 = 68;
pub const SYS_MSGSND: u64 = 69;
pub const SYS_MSGRCV: u64 = 70;
pub const SYS_MSGCTL: u64 = 71;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_FLOCK: u64 = 73;
pub const SYS_FSYNC: u64 = 74;
pub const SYS_FDATASYNC: u64 = 75;
pub const SYS_TRUNCATE: u64 = 76;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_GETDENTS: u64 = 78;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_FCHDIR: u64 = 81;
pub const SYS_RENAME: u64 = 82;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_CREAT: u64 = 85;
pub const SYS_LINK: u64 = 86;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_SYMLINK: u64 = 88;
pub const SYS_READLINK: u64 = 89;
pub const SYS_CHMOD: u64 = 90;
pub const SYS_FCHMOD: u64 = 91;
pub const SYS_CHOWN: u64 = 92;
pub const SYS_FCHOWN: u64 = 93;
pub const SYS_LCHOWN: u64 = 94;
pub const SYS_UMASK: u64 = 95;
pub const SYS_GETTIMEOFDAY: u64 = 96;
pub const SYS_GETRLIMIT: u64 = 97;
pub const SYS_GETRUSAGE: u64 = 98;
pub const SYS_SYSINFO: u64 = 99;
pub const SYS_TIMES: u64 = 100;
pub const SYS_PTRACE: u64 = 101;
pub const SYS_GETUID: u64 = 102;
pub const SYS_SYSLOG: u64 = 103;
pub const SYS_GETGID: u64 = 104;
pub const SYS_SETUID: u64 = 105;
pub const SYS_SETGID: u64 = 106;
pub const SYS_GETEUID: u64 = 107;
pub const SYS_GETEGID: u64 = 108;
pub const SYS_SETPGID: u64 = 109;
pub const SYS_GETPPID: u64 = 110;
pub const SYS_GETPGRP: u64 = 111;
pub const SYS_SETSID: u64 = 112;
pub const SYS_SETREUID: u64 = 113;
pub const SYS_SETREGID: u64 = 114;
pub const SYS_GETGROUPS: u64 = 115;
pub const SYS_SETGROUPS: u64 = 116;
pub const SYS_SETRESUID: u64 = 117;
pub const SYS_GETRESUID: u64 = 118;
pub const SYS_SETRESGID: u64 = 119;
pub const SYS_GETRESGID: u64 = 120;
pub const SYS_GETPGID: u64 = 121;
pub const SYS_SETFSUID: u64 = 122;
pub const SYS_SETFSGID: u64 = 123;
pub const SYS_GETSID: u64 = 124;
pub const SYS_CAPGET: u64 = 125;
pub const SYS_CAPSET: u64 = 126;
pub const SYS_RT_SIGPENDING: u64 = 127;
pub const SYS_RT_SIGTIMEDWAIT: u64 = 128;
pub const SYS_RT_SIGQUEUEINFO: u64 = 129;
pub const SYS_RT_SIGSUSPEND: u64 = 130;
pub const SYS_SIGALTSTACK: u64 = 131;
pub const SYS_UTIME: u64 = 132;
pub const SYS_MKNOD: u64 = 133;
pub const SYS_USELIB: u64 = 134;
pub const SYS_PERSONALITY: u64 = 135;
pub const SYS_USTAT: u64 = 136;
pub const SYS_STATFS: u64 = 137;
pub const SYS_FSTATFS: u64 = 138;
pub const SYS_SYSFS: u64 = 139;
pub const SYS_GETPRIORITY: u64 = 140;
pub const SYS_SETPRIORITY: u64 = 141;
pub const SYS_SCHED_SETPARAM: u64 = 142;
pub const SYS_SCHED_GETPARAM: u64 = 143;
pub const SYS_SCHED_SETSCHEDULER: u64 = 144;
pub const SYS_SCHED_GETSCHEDULER: u64 = 145;
pub const SYS_SCHED_GET_PRIORITY_MAX: u64 = 146;
pub const SYS_SCHED_GET_PRIORITY_MIN: u64 = 147;
pub const SYS_SCHED_RR_GET_INTERVAL: u64 = 148;
pub const SYS_MLOCK: u64 = 149;
pub const SYS_MUNLOCK: u64 = 150;
pub const SYS_MLOCKALL: u64 = 151;
pub const SYS_MUNLOCKALL: u64 = 152;
pub const SYS_VHANGUP: u64 = 153;
pub const SYS_MODIFY_LDT: u64 = 154;
pub const SYS_PIVOT_ROOT: u64 = 155;
pub const SYS_PRCTL: u64 = 157;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_ADJTIMEX: u64 = 159;
pub const SYS_SETRLIMIT: u64 = 160;
pub const SYS_CHROOT: u64 = 161;
pub const SYS_SYNC: u64 = 162;
pub const SYS_ACCT: u64 = 163;
pub const SYS_SETTIMEOFDAY: u64 = 164;
pub const SYS_MOUNT: u64 = 165;
pub const SYS_UMOUNT2: u64 = 166;
pub const SYS_SWAPON: u64 = 167;
pub const SYS_SWAPOFF: u64 = 168;
pub const SYS_REBOOT: u64 = 169;
pub const SYS_SETHOSTNAME: u64 = 170;
pub const SYS_SETDOMAINNAME: u64 = 171;
pub const SYS_IOPL: u64 = 172;
pub const SYS_IOPERM: u64 = 173;
pub const SYS_CREATE_MODULE: u64 = 174;
pub const SYS_INIT_MODULE: u64 = 175;
pub const SYS_DELETE_MODULE: u64 = 176;
pub const SYS_QUERY_MODULE: u64 = 177;
pub const SYS_QUOTACTL: u64 = 179;
pub const SYS_GETTID: u64 = 186;
pub const SYS_TKILL: u64 = 200;
pub const SYS_TIME: u64 = 201;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_SCHED_SETAFFINITY: u64 = 203;
pub const SYS_SCHED_GETAFFINITY: u64 = 204;
pub const SYS_EPOLL_CREATE: u64 = 213;
pub const SYS_GETDENTS64: u64 = 217;
pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_SEMTIMEDOP: u64 = 220;
pub const SYS_FADVISE64: u64 = 221;
pub const SYS_TIMER_CREATE: u64 = 222;
pub const SYS_TIMER_SETTIME: u64 = 223;
pub const SYS_TIMER_GETTIME: u64 = 224;
pub const SYS_TIMER_GETOVERRUN: u64 = 225;
pub const SYS_TIMER_DELETE: u64 = 226;
pub const SYS_CLOCK_SETTIME: u64 = 227;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_CLOCK_GETRES: u64 = 229;
pub const SYS_CLOCK_NANOSLEEP: u64 = 230;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_CTL: u64 = 233;
pub const SYS_TGKILL: u64 = 234;
pub const SYS_UTIMES: u64 = 235;
pub const SYS_WAITID: u64 = 247;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_MKDIRAT: u64 = 258;
pub const SYS_MKNODAT: u64 = 259;
pub const SYS_FCHOWNAT: u64 = 260;
pub const SYS_FUTIMESAT: u64 = 261;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_UNLINKAT: u64 = 263;
pub const SYS_RENAMEAT: u64 = 264;
pub const SYS_LINKAT: u64 = 265;
pub const SYS_SYMLINKAT: u64 = 266;
pub const SYS_READLINKAT: u64 = 267;
pub const SYS_FCHMODAT: u64 = 268;
pub const SYS_FACCESSAT: u64 = 269;
pub const SYS_PSELECT6: u64 = 270;
pub const SYS_PPOLL: u64 = 271;
pub const SYS_UNSHARE: u64 = 272;
pub const SYS_SPLICE: u64 = 275;
pub const SYS_TEE: u64 = 276;
pub const SYS_SYNC_FILE_RANGE: u64 = 277;
pub const SYS_VMSPLICE: u64 = 278;
pub const SYS_EPOLL_PWAIT: u64 = 281;
pub const SYS_EVENTFD: u64 = 284;
pub const SYS_FALLOCATE: u64 = 285;
pub const SYS_EVENTFD2: u64 = 290;
pub const SYS_EPOLL_CREATE1: u64 = 291;
pub const SYS_DUP3: u64 = 292;
pub const SYS_PIPE2: u64 = 293;
pub const SYS_INOTIFY_INIT1: u64 = 294;
pub const SYS_PREADV: u64 = 295;
pub const SYS_PWRITEV: u64 = 296;
pub const SYS_GETCPU: u64 = 309;
pub const SYS_RENAMEAT2: u64 = 316;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_COPY_FILE_RANGE: u64 = 326;
pub const SYS_PREADV2: u64 = 327;
pub const SYS_PWRITEV2: u64 = 328;
pub const SYS_STATX: u64 = 332;
pub const SYS_OPENAT2: u64 = 437;
pub const SYS_EPOLL_PWAIT2: u64 = 441;

pub const SYS_EXO_IPC_SEND: u64 = 300;
pub const SYS_EXO_IPC_RECV: u64 = 301;
pub const SYS_EXO_IPC_RECV_NB: u64 = 302;
pub const SYS_EXO_IPC_CALL: u64 = 303;
pub const SYS_EXO_IPC_CREATE: u64 = 304;
pub const SYS_EXO_IPC_DESTROY: u64 = 305;
pub const SYS_EXO_IPC_LOOKUP: u64 = 306;
pub const SYS_EXO_MEM_COPY_FROM_PID: u64 = 307;
pub const SYS_EXO_MEM_COPY_TO_PID: u64 = 308;
pub const SYS_EXO_MEM_SHARE: u64 = 310;
pub const SYS_EXO_MEM_REVOKE: u64 = 311;
pub const SYS_EXO_MEM_MAP_PID: u64 = 312;
pub const SYS_EXO_MEM_MUNMAP_PID: u64 = 313;
pub const SYS_EXO_MEM_MPROTECT_PID: u64 = 314;
pub const SYS_EXO_CAP_CREATE: u64 = 320;
pub const SYS_EXO_CAP_DELEGATE: u64 = 321;
pub const SYS_EXO_CAP_REVOKE: u64 = 322;
pub const SYS_EXO_CAP_CHECK: u64 = 323;
pub const SYS_EXO_PERF_READ: u64 = 330;
pub const SYS_EXO_PERF_ENABLE: u64 = 331;
pub const SYS_EXO_PERF_DISABLE: u64 = 332;
pub const SYS_EXO_DEBUG_ATTACH: u64 = 340;
pub const SYS_EXO_DEBUG_REGS: u64 = 341;
pub const SYS_EXO_LOG: u64 = 350;
pub const SYS_EXO_PROCESS_LIST: u64 = 351;
pub const SYS_EXO_PHOENIX_STATE_SET: u64 = 352;
pub const SYS_EXO_PHOENIX_STATE_GET: u64 = 353;
pub const SYS_EXO_BPF: u64 = 360;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExoPhoenixStateWire {
    Normal = 1,
    NetworkDraining = 9,
    NetworkSerialized = 10,
}

impl ExoPhoenixStateWire {
    #[inline(always)]
    pub const fn as_syscall_arg(self) -> u64 {
        self as u8 as u64
    }
}

pub const EXO_PHOENIX_STATE_NORMAL: u64 = ExoPhoenixStateWire::Normal.as_syscall_arg();
pub const EXO_PHOENIX_STATE_NETWORK_DRAINING: u64 =
    ExoPhoenixStateWire::NetworkDraining.as_syscall_arg();
pub const EXO_PHOENIX_STATE_NETWORK_SERIALIZED: u64 =
    ExoPhoenixStateWire::NetworkSerialized.as_syscall_arg();

pub const SYS_EXOFS_PATH_RESOLVE: u64 = 500;
pub const SYS_EXOFS_OBJECT_OPEN: u64 = 501;
pub const SYS_EXOFS_OBJECT_READ: u64 = 502;
pub const SYS_EXOFS_OBJECT_WRITE: u64 = 503;
pub const SYS_EXOFS_OBJECT_CREATE: u64 = 504;
pub const SYS_EXOFS_OBJECT_DELETE: u64 = 505;
pub const SYS_EXOFS_OBJECT_STAT: u64 = 506;
pub const SYS_EXOFS_OBJECT_SET_META: u64 = 507;
pub const SYS_EXOFS_GET_CONTENT_HASH: u64 = 508;
pub const SYS_EXOFS_SNAPSHOT_CREATE: u64 = 509;
pub const SYS_EXOFS_SNAPSHOT_LIST: u64 = 510;
pub const SYS_EXOFS_SNAPSHOT_MOUNT: u64 = 511;
pub const SYS_EXOFS_RELATION_CREATE: u64 = 512;
pub const SYS_EXOFS_RELATION_QUERY: u64 = 513;
pub const SYS_EXOFS_GC_TRIGGER: u64 = 514;
pub const SYS_EXOFS_QUOTA_QUERY: u64 = 515;
pub const SYS_EXOFS_EXPORT_OBJECT: u64 = 516;
pub const SYS_EXOFS_IMPORT_OBJECT: u64 = 517;
pub const SYS_EXOFS_EPOCH_COMMIT: u64 = 518;
pub const SYS_EXOFS_OPEN_BY_PATH: u64 = 519;
pub const SYS_EXOFS_READDIR: u64 = 520;
pub const SYS_EXOFS_FIRST: u64 = SYS_EXOFS_PATH_RESOLVE;
pub const SYS_EXOFS_LAST: u64 = SYS_EXOFS_READDIR;
pub const SYS_EXOFS_COUNT: u64 = SYS_EXOFS_LAST - SYS_EXOFS_FIRST + 1;

pub const SYS_IPC_REGISTER: u64 = SYS_EXO_IPC_CREATE;
pub const SYS_IPC_RECV: u64 = SYS_EXO_IPC_RECV;
pub const SYS_IPC_SEND: u64 = SYS_EXO_IPC_SEND;
pub const SYS_IPC_LOOKUP: u64 = SYS_EXO_IPC_LOOKUP;
pub const SYS_PROC_CLONE: u64 = SYS_FORK;
pub const SYS_PROC_EXEC: u64 = SYS_EXECVE;

pub const IPC_HEADER_SIZE: usize = 8;
pub const IPC_CAP_TOKEN_SIZE: usize = 20;
pub const IPC_INLINE_PAYLOAD_SIZE: usize = 192;
pub const IPC_ENVELOPE_SIZE: usize = IPC_HEADER_SIZE + IPC_INLINE_PAYLOAD_SIZE;
pub const IPC_CAP_TOKEN_PAYLOAD_OFFSET: usize = IPC_INLINE_PAYLOAD_SIZE - IPC_CAP_TOKEN_SIZE;
pub const IPC_CAP_TOKEN_OFFSET: usize = IPC_HEADER_SIZE + IPC_CAP_TOKEN_PAYLOAD_OFFSET;
pub const IPC_KERNEL_MAX_MSG_SIZE: usize = 240;
pub const IPC_RECV_MAX_LEN: usize = 65_536;

const _: () = assert!(
    IPC_INLINE_PAYLOAD_SIZE <= 200,
    "IPC_INLINE_PAYLOAD_SIZE must stay within the v0.2 inline budget"
);
const _: () = assert!(
    IPC_ENVELOPE_SIZE == 200,
    "IPC_ENVELOPE_SIZE must be the canonical 200-byte ABI envelope"
);
const _: () = assert!(
    IPC_ENVELOPE_SIZE <= IPC_KERNEL_MAX_MSG_SIZE,
    "IPC_ENVELOPE_SIZE must fit in the kernel raw IPC slot"
);
const _: () = assert!(
    IPC_CAP_TOKEN_OFFSET + IPC_CAP_TOKEN_SIZE == IPC_ENVELOPE_SIZE,
    "ExoCapTokenWire must be stored at the end of the IPC envelope"
);

/// Convention canonique: `SYS_EXO_IPC_RECV(endpoint, buf, len, flags)`.
/// Compatibilite legacy: quand `flags == 0` et que le premier argument ressemble
/// a un pointeur userspace, le kernel accepte encore `recv(buf, len, flags)`.
/// Les nouveaux serveurs doivent toujours utiliser la forme canonique.
pub const IPC_RECV_LEGACY_SHORTHAND_MAX_LEN: usize = IPC_RECV_MAX_LEN;

pub const EXO_PROCESS_NAME_LEN: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExoProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub state: u32,
    pub threads: u32,
    pub name: [u8; EXO_PROCESS_NAME_LEN],
    pub utime_ns: u64,
    pub stime_ns: u64,
}

impl ExoProcessInfo {
    #[inline(always)]
    pub const fn zeroed() -> Self {
        Self {
            pid: 0,
            ppid: 0,
            state: 0,
            threads: 0,
            name: [0u8; EXO_PROCESS_NAME_LEN],
            utime_ns: 0,
            stime_ns: 0,
        }
    }
}

impl Default for ExoProcessInfo {
    #[inline(always)]
    fn default() -> Self {
        Self::zeroed()
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IpcMessage {
    pub sender_pid: u32,
    pub msg_type: u32,
    pub payload: [u8; IPC_INLINE_PAYLOAD_SIZE],
}

impl IpcMessage {
    #[inline(always)]
    pub const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0u8; IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

impl Default for IpcMessage {
    #[inline(always)]
    fn default() -> Self {
        Self::zeroed()
    }
}

const _: () = assert!(core::mem::size_of::<IpcMessage>() == IPC_ENVELOPE_SIZE);
const _: () = assert!(core::mem::offset_of!(IpcMessage, payload) == IPC_HEADER_SIZE);

pub const EXO_CAP_TOKEN_WIRE_SIZE: usize = IPC_CAP_TOKEN_SIZE;

pub const EXO_CAP_TYPE_IPC_ENDPOINT: u32 = 1;

pub const EXO_CAP_RIGHT_IPC_CONNECT: u32 = 1 << 6;
pub const EXO_CAP_RIGHT_IPC_SEND: u32 = 1 << 7;
pub const EXO_CAP_RIGHT_IPC_RECV: u32 = 1 << 8;
pub const EXO_CAP_RIGHT_IPC_MANAGE: u32 = 1 << 9;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ExoCapTokenWire {
    pub bytes: [u8; EXO_CAP_TOKEN_WIRE_SIZE],
}

impl ExoCapTokenWire {
    #[inline(always)]
    pub const fn empty() -> Self {
        Self {
            bytes: [0u8; EXO_CAP_TOKEN_WIRE_SIZE],
        }
    }

    #[inline(always)]
    pub fn object_id(self) -> u64 {
        u64::from_ne_bytes([
            self.bytes[0],
            self.bytes[1],
            self.bytes[2],
            self.bytes[3],
            self.bytes[4],
            self.bytes[5],
            self.bytes[6],
            self.bytes[7],
        ])
    }

    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.object_id() == 0
    }
}

#[inline(always)]
pub unsafe fn exo_cap_create(
    cap_type: u32,
    rights: u32,
    target_pid: u32,
    token_out: &mut ExoCapTokenWire,
) -> i64 {
    unsafe {
        syscall4(
            SYS_EXO_CAP_CREATE,
            cap_type as u64,
            rights as u64,
            target_pid as u64,
            token_out as *mut ExoCapTokenWire as u64,
        )
    }
}

#[inline(always)]
pub unsafe fn exo_cap_check(
    token: &ExoCapTokenWire,
    required_rights: u32,
    target_pid: u32,
    expected_type: u32,
) -> i64 {
    unsafe {
        syscall4(
            SYS_EXO_CAP_CHECK,
            token as *const ExoCapTokenWire as u64,
            required_rights as u64,
            target_pid as u64,
            expected_type as u64,
        )
    }
}

pub const EXOFS_RIGHT_READ: u32 = 1 << 0;
pub const EXOFS_RIGHT_WRITE: u32 = 1 << 1;
pub const EXOFS_RIGHT_CREATE: u32 = 1 << 2;
pub const EXOFS_RIGHT_DELETE: u32 = 1 << 3;
pub const EXOFS_RIGHT_STAT: u32 = 1 << 4;
pub const EXOFS_RIGHT_SETMETA: u32 = 1 << 5;
pub const EXOFS_RIGHT_LIST: u32 = 1 << 6;
pub const EXOFS_RIGHT_EXEC: u32 = 1 << 7;
pub const EXOFS_RIGHT_CHOWN: u32 = 1 << 8;
pub const EXOFS_RIGHT_CHMOD: u32 = 1 << 9;
pub const EXOFS_RIGHT_INSPECT_CONTENT: u32 = 1 << 10;
pub const EXOFS_RIGHT_SNAPSHOT_CREATE: u32 = 1 << 11;
pub const EXOFS_RIGHT_RELATION_CREATE: u32 = 1 << 12;
pub const EXOFS_RIGHT_GC_TRIGGER: u32 = 1 << 13;
pub const EXOFS_RIGHT_EXPORT: u32 = 1 << 14;
pub const EXOFS_RIGHT_IMPORT: u32 = 1 << 15;
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

#[repr(C)]
#[derive(Clone, Copy, Default)]
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

impl ExofsPathResolveResult {
    #[inline(always)]
    pub fn blob_id_low64(&self) -> u64 {
        u64::from_le_bytes([
            self.blob_id[0],
            self.blob_id[1],
            self.blob_id[2],
            self.blob_id[3],
            self.blob_id[4],
            self.blob_id[5],
            self.blob_id[6],
            self.blob_id[7],
        ])
    }
}

const _: () = assert!(core::mem::size_of::<ExofsPathResolveResult>() == 104);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ExofsOpenArgs {
    pub flags: u32,
    pub mode: u32,
    pub epoch_id: u64,
    pub owner_uid: u64,
    pub size_hint: u64,
    pub _reserved: [u64; 2],
}

const _: () = assert!(core::mem::size_of::<ExofsOpenArgs>() == 48);

#[inline(always)]
pub unsafe fn exofs_path_resolve_raw(
    path_ptr: u64,
    path_len: u64,
    flags: u64,
    out: *mut ExofsPathResolveResult,
    cap_rights: u64,
) -> i64 {
    unsafe {
        syscall6(
            SYS_EXOFS_PATH_RESOLVE,
            path_ptr,
            path_len,
            flags,
            out as u64,
            0,
            cap_rights,
        )
    }
}

#[inline(always)]
pub unsafe fn exofs_object_open_raw(
    path_ptr: u64,
    path_len: u64,
    flags: u64,
    out_fd: *mut u32,
    args: *const ExofsOpenArgs,
    cap_rights: u64,
) -> i64 {
    unsafe {
        syscall6(
            SYS_EXOFS_OBJECT_OPEN,
            path_ptr,
            path_len,
            flags,
            out_fd as u64,
            args as u64,
            cap_rights,
        )
    }
}

#[inline(always)]
pub unsafe fn exofs_open_by_path_raw(path_ptr: u64, flags: u64, mode: u64, cap_rights: u64) -> i64 {
    unsafe {
        syscall6(
            SYS_EXOFS_OPEN_BY_PATH,
            path_ptr,
            flags,
            mode,
            0,
            0,
            cap_rights,
        )
    }
}

#[inline(always)]
pub unsafe fn exofs_readdir_raw(fd: u64, buf_ptr: u64, buf_len: u64, cap_rights: u64) -> i64 {
    unsafe { syscall6(SYS_EXOFS_READDIR, fd, buf_ptr, buf_len, 0, 0, cap_rights) }
}

pub const SYS_IRQ_REGISTER: u64 = 530;
pub const SYS_IRQ_ACK: u64 = 531;
pub const SYS_MMIO_MAP: u64 = 532;
pub const SYS_MMIO_UNMAP: u64 = 533;
pub const SYS_DMA_ALLOC: u64 = 534;
pub const SYS_DMA_FREE: u64 = 535;
pub const SYS_DMA_SYNC: u64 = 536;
pub const SYS_PCI_CFG_READ: u64 = 537;
pub const SYS_PCI_CFG_WRITE: u64 = 538;
pub const SYS_PCI_BUS_MASTER: u64 = 539;
pub const SYS_PCI_CLAIM: u64 = 540;
pub const SYS_DMA_MAP: u64 = 541;
pub const SYS_DMA_UNMAP: u64 = 542;
pub const SYS_MSI_ALLOC: u64 = 543;
pub const SYS_MSI_CONFIG: u64 = 544;
pub const SYS_MSI_FREE: u64 = 545;
pub const SYS_PCI_SET_TOPOLOGY: u64 = 546;

pub const O_RDONLY: u64 = 0;
pub const O_WRONLY: u64 = 1;
pub const O_RDWR: u64 = 2;
pub const O_CREAT: u64 = 0x0040;
pub const O_EXCL: u64 = 0x0080;
pub const O_TRUNC: u64 = 0x0200;
pub const O_APPEND: u64 = 0x0400;
pub const O_NONBLOCK: u64 = 0x0800;
pub const O_CLOEXEC: u64 = 0x0008_0000;

pub const SYNC_FILE_RANGE_WRITE: u32 = 1;
pub const SYNC_FILE_RANGE_WAIT_BEFORE: u32 = 2;
pub const SYNC_FILE_RANGE_WAIT_AFTER: u32 = 4;

pub const FALLOC_FL_KEEP_SIZE: u32 = 0x01;
pub const FALLOC_FL_PUNCH_HOLE: u32 = 0x02;
pub const FALLOC_FL_COLLAPSE_RANGE: u32 = 0x08;
pub const FALLOC_FL_ZERO_RANGE: u32 = 0x10;
pub const FALLOC_FL_INSERT_RANGE: u32 = 0x20;
pub const FALLOC_FL_UNSHARE_RANGE: u32 = 0x40;

pub const RENAME_NOREPLACE: u32 = 1;
pub const RENAME_EXCHANGE: u32 = 2;
pub const RENAME_WHITEOUT: u32 = 4;

pub const STATX_TYPE: u32 = 0x0000_0001;
pub const STATX_MODE: u32 = 0x0000_0002;
pub const STATX_NLINK: u32 = 0x0000_0004;
pub const STATX_UID: u32 = 0x0000_0008;
pub const STATX_GID: u32 = 0x0000_0010;
pub const STATX_ATIME: u32 = 0x0000_0020;
pub const STATX_MTIME: u32 = 0x0000_0040;
pub const STATX_CTIME: u32 = 0x0000_0080;
pub const STATX_INO: u32 = 0x0000_0100;
pub const STATX_SIZE: u32 = 0x0000_0200;
pub const STATX_BLOCKS: u32 = 0x0000_0400;
pub const STATX_BASIC_STATS: u32 = 0x0000_07ff;
pub const STATX_BTIME: u32 = 0x0000_0800;
pub const STATX_ATTR_IMMUTABLE: u64 = 0x0000_0010;
pub const STATX_ATTR_VERITY: u64 = 0x0010_0000;

pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;
pub const EPOLL_CLOEXEC: i32 = 0x0008_0000;

pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;

pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_FIXED: u64 = 0x10;
pub const MAP_ANONYMOUS: u64 = 0x20;

pub const IPC_FLAG_TIMEOUT: u64 = 0x0001;
pub const IPC_FLAG_INJECT_SRC_PID: u64 = 0x0002;
pub const WNOHANG: u64 = 1;
pub const SA_RESTART: u64 = 0x10000000;
pub const EPERM: i64 = -1;
pub const ENOENT: i64 = -2;
pub const EINTR: i64 = -4;
pub const EIO: i64 = -5;
pub const E2BIG: i64 = -7;
pub const EMSGSIZE: i64 = -90;
pub const EBADF: i64 = -9;
pub const EAGAIN: i64 = -11;
pub const ENOMEM: i64 = -12;
pub const EACCES: i64 = -13;
pub const EFAULT: i64 = -14;
pub const EBUSY: i64 = -16;
pub const EEXIST: i64 = -17;
pub const ENOTDIR: i64 = -20;
pub const EISDIR: i64 = -21;
pub const ENODEV: i64 = -19;
pub const EINVAL: i64 = -22;
pub const EMFILE: i64 = -24;
pub const EPIPE: i64 = -32;
pub const ENOSPC: i64 = -28;
pub const EAFNOSUPPORT: i64 = -97;
pub const EADDRINUSE: i64 = -98;
pub const ENOTSUP: i64 = -95;
pub const EOPNOTSUPP: i64 = -95;
pub const ENOBUFS: i64 = -105;
pub const ENOTCONN: i64 = -107;
pub const ENETDOWN: i64 = -100;
pub const ENETUNREACH: i64 = -101;
pub const ENOSYS: i64 = -38;
pub const ETIMEDOUT: i64 = -110;
