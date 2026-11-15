//! # syscall/numbers.rs — Exo-OS system call numbers
//!
//! Defines numeric constants for all supported syscalls.
//!
//! ## Compatibility
//! - [0..299]   : Linux x86_64 compatible numbers (same ABI as glibc)
//! - [300..399] : Exo-OS extensions (native IPC, capabilities, security)
//! - [400..436] : reserved for future use
//! - [437]      : SYS_OPENAT2 (Linux compat)
//! - [438..440] : reserved for future use
//! - [441]      : SYS_EPOLL_PWAIT2 (Linux compat)
//! - [442..499] : reserved for future use
//! - [500..520] : Native ExoFS syscalls
//! - [521]      : boot framebuffer information for Ring1 fb_server
//! - [522..529] : reserved for future use
//! - [530..549] : GI-03 drivers syscalls
//! - 550        : SYSCALL_TABLE_SIZE (total table size)
//!
//! ## Architectural Rule
//! Linux numbers are kept identical to allow unpatched
//! musl/glibc libc. Native Exo-OS syscalls start at 300
//! to avoid any conflict with future Linux additions.
//!
//! ## Register ABI (Linux/Exo-OS 64-bit)
//! ```
//! rax = syscall number (in) / return value (out)
//! rdi = arg1   rsi = arg2   rdx = arg3
//! r10 = arg4   r8  = arg5   r9  = arg6
//! rcx = return RIP (saved by hardware SYSCALL)
//! r11 = RFLAGS (saved by hardware SYSCALL)
//! ```

// ─────────────────────────────────────────────────────────────────────────────
// Total dispatch table size
// ─────────────────────────────────────────────────────────────────────────────

/// Syscall table size (one slot per possible number).
/// 550 = covers POSIX (0–499) + ExoFS (500–520) + framebuffer (521)
/// + GI-03 drivers (530–549).
pub const SYSCALL_TABLE_SIZE: usize = 550;

/// Invalid number (returns -ENOSYS)
pub const SYSCALL_INVALID: u64 = u64::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// Block 0–99: I/O, Files, Memory (Linux-compatible)
// ─────────────────────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────────────────────
// Block 100–199: Time, Processes, Signals (Linux-compatible)
// ─────────────────────────────────────────────────────────────────────────────

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
pub const SYS_UTIMENSAT: u64 = 280;
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
pub const SYS_GETCPU: u64 = 309; // conflict: remapped to 298 on Linux side, see compat
pub const SYS_RENAMEAT2: u64 = 316;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_MEMFD_CREATE: u64 = 319;
pub const SYS_COPY_FILE_RANGE: u64 = 326;
pub const SYS_PREADV2: u64 = 327;
pub const SYS_PWRITEV2: u64 = 328;
pub const SYS_STATX: u64 = 332;
pub const SYS_OPENAT2: u64 = 437;
pub const SYS_EPOLL_PWAIT2: u64 = 441;

// ─────────────────────────────────────────────────────────────────────────────
// Block 300–399: Native Exo-OS syscalls
// ─────────────────────────────────────────────────────────────────────────────

/// Send a native Exo-OS IPC message
pub const SYS_EXO_IPC_SEND: u64 = 300;
/// Receive a native Exo-OS IPC message
pub const SYS_EXO_IPC_RECV: u64 = 301;
/// Receive in non-blocking mode
pub const SYS_EXO_IPC_RECV_NB: u64 = 302;
/// Synchronous IPC call (atomic send + recv)
pub const SYS_EXO_IPC_CALL: u64 = 303;
/// Create an IPC endpoint
pub const SYS_EXO_IPC_CREATE: u64 = 304;
/// Destroy an IPC endpoint
pub const SYS_EXO_IPC_DESTROY: u64 = 305;
/// Resolve an IPC endpoint by name
pub const SYS_EXO_IPC_LOOKUP: u64 = 306;
/// Copy from the address space of a target PID to calling process
pub const SYS_EXO_MEM_COPY_FROM_PID: u64 = 307;
/// Copy from calling process to the address space of a target PID
pub const SYS_EXO_MEM_COPY_TO_PID: u64 = 308;
/// Share a memory page via capability
pub const SYS_EXO_MEM_SHARE: u64 = 310;
/// Revoke a memory share
pub const SYS_EXO_MEM_REVOKE: u64 = 311;
/// Map a VMA into target PID space from memory_server
pub const SYS_EXO_MEM_MAP_PID: u64 = 312;
/// Unmap a VMA from target PID space from memory_server
pub const SYS_EXO_MEM_MUNMAP_PID: u64 = 313;
/// Modify VMA protections in target PID space from memory_server
pub const SYS_EXO_MEM_MPROTECT_PID: u64 = 314;
/// Obtain a capability
pub const SYS_EXO_CAP_CREATE: u64 = 320;
/// Delegate a capability to another process
pub const SYS_EXO_CAP_DELEGATE: u64 = 321;
/// Revoke a capability
pub const SYS_EXO_CAP_REVOKE: u64 = 322;
/// Check if a capability is valid
pub const SYS_EXO_CAP_CHECK: u64 = 323;
/// Irreversibly restrict current process (OpenBSD pledge-like sandbox).
/// `promises` = bitmask of `pledge_flags`; unpromised capabilities become
/// zero-trust restrictions enforced at the syscall boundary (TIER 2.10).
pub const SYS_EXO_PLEDGE: u64 = 324;
/// Drain the security event feed kernel→exo_shield (TIER 3.1).
/// Reserved for exo_shield server (gated on its service class). Copies up to
/// `count` `ShieldEvent`s into `buf`; returns the number of drained events.
pub const SYS_EXO_SHIELD_DRAIN: u64 = 325;
/// Read a kernel performance counter
pub const SYS_EXO_PERF_READ: u64 = 342;
/// Enable performance events
pub const SYS_EXO_PERF_ENABLE: u64 = 343;
/// Disable performance events
pub const SYS_EXO_PERF_DISABLE: u64 = 344;

pub const EXO_PERF_SYSCALL_COUNT: u64 = 1;
pub const EXO_PERF_IPC_MESSAGES_SENT: u64 = 2;
pub const EXO_PERF_IPC_MESSAGES_RECEIVED: u64 = 3;
pub const EXO_PERF_IPC_MESSAGES_DROPPED: u64 = 4;
pub const EXO_PERF_IPC_RPC_CALLS: u64 = 5;
pub const EXO_PERF_IPC_RPC_TIMEOUTS: u64 = 6;
pub const EXO_PERF_IPC_FUTEX_WAITS: u64 = 7;
pub const EXO_PERF_IPC_FUTEX_WAKES: u64 = 8;
pub const EXO_PERF_DISPATCH_TOTAL: u64 = 9;
pub const EXO_PERF_DISPATCH_FAST_PATH: u64 = 10;
pub const EXO_PERF_DISPATCH_SLOW_PATH: u64 = 11;
pub const EXO_PERF_DISPATCH_ENOSYS: u64 = 12;
/// Enable/disable process debug mode (Ring 0 only)
pub const SYS_EXO_DEBUG_ATTACH: u64 = 340;
/// Read registers of a debugged process
pub const SYS_EXO_DEBUG_REGS: u64 = 341;
/// Direct kernel log (Ring 0 permissions required)
pub const SYS_EXO_LOG: u64 = 350;
/// List living processes from kernel registry
pub const SYS_EXO_PROCESS_LIST: u64 = 351;
/// Synchronize global Phoenix state from a root server
pub const SYS_EXO_PHOENIX_STATE_SET: u64 = 352;
/// Read global Phoenix state
pub const SYS_EXO_PHOENIX_STATE_GET: u64 = 353;
/// Exo-OS eBPF probe
pub const SYS_EXO_BPF: u64 = 360;

// ─────────────────────────────────────────────────────────────────────────────
// Block 500–518: Native ExoFS (ExoOS object filesystem)
// ─────────────────────────────────────────────────────────────────────────────
// RULE SYS-10: Syscalls 0-499 = standard POSIX. Syscalls 500-518 = native ExoFS.
// These constants are PUBLIC — used by exo-libc AND exo-rt.

/// ExoFS: path resolution → ObjectId   (path → BlobId + ObjectId)
pub const SYS_EXOFS_PATH_RESOLVE: u64 = 500;
/// ExoFS: open existing object         (ObjectId + rights → fd)
pub const SYS_EXOFS_OBJECT_OPEN: u64 = 501;
/// ExoFS: read object                  (fd, offset, buf, len)
pub const SYS_EXOFS_OBJECT_READ: u64 = 502;
/// ExoFS: write to object              (fd, offset, buf, len)
pub const SYS_EXOFS_OBJECT_WRITE: u64 = 503;
/// ExoFS: create new object
pub const SYS_EXOFS_OBJECT_CREATE: u64 = 504;
/// ExoFS: delete object
pub const SYS_EXOFS_OBJECT_DELETE: u64 = 505;
/// ExoFS: object metadata (stat-like)
pub const SYS_EXOFS_OBJECT_STAT: u64 = 506;
/// ExoFS: update metadata
pub const SYS_EXOFS_OBJECT_SET_META: u64 = 507;
/// ExoFS: content hash (audited SEC-09 — copy_from_user mandatory)
pub const SYS_EXOFS_GET_CONTENT_HASH: u64 = 508;
/// ExoFS: create snapshot
pub const SYS_EXOFS_SNAPSHOT_CREATE: u64 = 509;
/// ExoFS: list available snapshots
pub const SYS_EXOFS_SNAPSHOT_LIST: u64 = 510;
/// ExoFS: mount snapshot in read-only mode
pub const SYS_EXOFS_SNAPSHOT_MOUNT: u64 = 511;
/// ExoFS: create relation between objects
pub const SYS_EXOFS_RELATION_CREATE: u64 = 512;
/// ExoFS: query relations
pub const SYS_EXOFS_RELATION_QUERY: u64 = 513;
/// ExoFS: manual GC trigger (garbage collector)
pub const SYS_EXOFS_GC_TRIGGER: u64 = 514;
/// ExoFS: capability quota query
pub const SYS_EXOFS_QUOTA_QUERY: u64 = 515;
/// ExoFS: export object to userspace
pub const SYS_EXOFS_EXPORT_OBJECT: u64 = 516;
/// ExoFS: import object from userspace
pub const SYS_EXOFS_IMPORT_OBJECT: u64 = 517;
/// ExoFS: epoch commit (3 NVMe barriers — guaranteed atomicity)
pub const SYS_EXOFS_EPOCH_COMMIT: u64 = 518;

// ─────────────────────────────────────────────────────────────────────────────
// Block 519–520: ExoFS Extensions — fixes BUG-01/BUG-02
// ─────────────────────────────────────────────────────────────────────────────

/// FIX BUG-01: combined Ring0 POSIX open() — chains path_resolve() + object_open()
/// atomically. Used by musl-exo: #define __NR_open 519
/// Signature: (path_ptr, path_len, flags, mode) → fd
pub const SYS_EXOFS_OPEN_BY_PATH: u64 = 519;

/// FIX BUG-02: ExoFS getdents64 — lists directory content.
/// Used by ls, find, opendir(). Without this syscall: ls/find/opendir() impossible.
/// Signature: (fd, buf_ptr, buf_len) → bytes populated
pub const SYS_EXOFS_READDIR: u64 = 520;

/// Terminal v0.2: exposes boot framebuffer info to Ring1 server.
/// Signature: (out_ptr: *mut FramebufferInfoWire) -> 0
pub const SYS_FRAMEBUFFER_INFO: u64 = 521;

// ─────────────────────────────────────────────────────────────────────────────
// Block 530–549: GI-03 Drivers (IRQ / DMA / PCI / IOMMU / I/O ports)
// ─────────────────────────────────────────────────────────────────────────────

/// GI-03: canonical IRQ routing registration.
pub const SYS_IRQ_REGISTER: u64 = 530;
/// GI-03: processed IRQ acknowledgement.
pub const SYS_IRQ_ACK: u64 = 531;
/// GI-03: MMIO mapping (calling PID, claim required).
pub const SYS_MMIO_MAP: u64 = 532;
/// GI-03: MMIO unmapping (calling PID).
pub const SYS_MMIO_UNMAP: u64 = 533;
/// GI-03: DMA allocation (returns IOVA, optional CPU virt via out ptr).
pub const SYS_DMA_ALLOC: u64 = 534;
/// GI-03: free allocated DMA.
pub const SYS_DMA_FREE: u64 = 535;
/// GI-03: CPU/device DMA synchronization.
pub const SYS_DMA_SYNC: u64 = 536;
/// GI-03: PCI config read (device claimed by calling PID).
pub const SYS_PCI_CFG_READ: u64 = 537;
/// GI-03: PCI config write (device claimed by calling PID).
pub const SYS_PCI_CFG_WRITE: u64 = 538;
/// GI-03: PCI Bus Master control (device claimed by calling PID).
pub const SYS_PCI_BUS_MASTER: u64 = 539;
/// GI-03: secure PCI claim (CORR-32).
pub const SYS_PCI_CLAIM: u64 = 540;
/// GI-03: buffer DMA map to IOVA.
pub const SYS_DMA_MAP: u64 = 541;
/// GI-03: DMA IOVA unmap.
pub const SYS_DMA_UNMAP: u64 = 542;
/// GI-03: MSI/MSI-X allocation (opaque handle).
pub const SYS_MSI_ALLOC: u64 = 543;
/// GI-03: MSI configuration by vector index.
pub const SYS_MSI_CONFIG: u64 = 544;
/// GI-03: free MSI handle.
pub const SYS_MSI_FREE: u64 = 545;
/// GI-03: PCI topology association (owner/parenting).
pub const SYS_PCI_SET_TOPOLOGY: u64 = 546;
/// GI-03: controlled PCI device discovery by vendor/device/class.
pub const SYS_PCI_FIND_DEVICE: u64 = 547;
/// GI-03: bounded I/O port read for authorized Ring1 drivers.
pub const SYS_IOPORT_READ: u64 = 548;
/// GI-03: bounded I/O port write for authorized Ring1 drivers.
pub const SYS_IOPORT_WRITE: u64 = 549;

// ─────────────────────────────────────────────────────────────────────────────
// FIX BUG-03: Process aliases for exo-rt
// ─────────────────────────────────────────────────────────────────────────────
// exo-rt references SYS_PROC_CLONE and SYS_PROC_EXEC but numbers.rs listed
// fork=57 and execve=59 without aliases. These constants are MANDATORY for
// exo-rt to compile.

/// exo-rt alias for fork() with kernel CoW — identical to SYS_FORK (57).
pub const SYS_PROC_CLONE: u64 = SYS_FORK;
/// exo-rt alias for execve() — identical to SYS_EXECVE (59).
pub const SYS_PROC_EXEC: u64 = SYS_EXECVE;

// ─────────────────────────────────────────────────────────────────────────────
// Exo-OS error codes (Linux errno compatible)
// ─────────────────────────────────────────────────────────────────────────────

/// Operation not supported
pub const ENOSYS: i64 = -38;
/// Invalid argument
pub const EINVAL: i64 = -22;
/// Permission denied
pub const EACCES: i64 = -13;
/// Bad address / invalid pointer
pub const EFAULT: i64 = -14;
/// Argument list too long
pub const E2BIG: i64 = -7;
pub const EMSGSIZE: i64 = -90;
/// Device or resource busy
pub const EBUSY: i64 = -16;
/// Interrupted by signal
pub const EINTR: i64 = -4;
/// Bad file descriptor
pub const EBADF: i64 = -9;
/// No such file or directory
pub const ENOENT: i64 = -2;
/// Out of memory
pub const ENOMEM: i64 = -12;
/// Resource temporarily unavailable (would block)
pub const EAGAIN: i64 = -11;
/// Too many open files
pub const EMFILE: i64 = -24;
/// Operation not permitted / capability denied
pub const EPERM: i64 = -1;
/// Value too large for defined data type
pub const EOVERFLOW: i64 = -75;
/// File exists
pub const EEXIST: i64 = -17;
/// Not a directory
pub const ENOTDIR: i64 = -20;
/// Is a directory
pub const EISDIR: i64 = -21;
/// No space left on device
pub const ENOSPC: i64 = -28;
/// Operation not supported
pub const ENOTSUP: i64 = -95;
/// POSIX/Linux alias of ENOTSUP
pub const EOPNOTSUPP: i64 = -95;
/// Illegal seek (pipe, socket, FIFO)
pub const ESPIPE: i64 = -29;

// ─────────────────────────────────────────────────────────────────────────────
// Classification helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Checks if a syscall number is in the Linux-compatible range
#[inline(always)]
pub const fn is_linux_compat(nr: u64) -> bool {
    nr < 300
        || matches!(
            nr,
            SYS_GETCPU
                | SYS_RENAMEAT2
                | SYS_GETRANDOM
                | SYS_MEMFD_CREATE
                | SYS_COPY_FILE_RANGE
                | SYS_PREADV2
                | SYS_PWRITEV2
                | SYS_STATX
                | SYS_OPENAT2
                | SYS_EPOLL_PWAIT2
        )
}

/// Checks if a syscall number is native Exo-OS
#[inline(always)]
pub const fn is_exoos_native(nr: u64) -> bool {
    nr >= 300 && nr < 400 && !is_linux_compat(nr)
}

/// Checks if a number is within the valid table range
#[inline(always)]
pub const fn is_valid_syscall(nr: u64) -> bool {
    (nr as usize) < SYSCALL_TABLE_SIZE
}

/// Checks if a syscall number is native ExoFS (500–520)
#[inline(always)]
pub const fn is_exofs_syscall(nr: u64) -> bool {
    nr >= SYS_EXOFS_PATH_RESOLVE && nr <= SYS_EXOFS_READDIR
}

/// Syscall numbers eligible for fast-path (<100 cycles, no alloc, no-lock)
/// Used by dispatch.rs to bypass the main table.
pub const FAST_PATH_SYSCALLS: &[u64] = &[
    SYS_GETPID,
    SYS_GETTID,
    SYS_GETUID,
    SYS_GETEUID,
    SYS_GETGID,
    SYS_GETEGID,
    SYS_GETPPID,
    SYS_CLOCK_GETTIME,
    SYS_GETTIMEOFDAY,
    SYS_GETCPU,
    SYS_SCHED_YIELD,
];

// ─────────────────────────────────────────────────────────────────────────────
// FIX-KRN-11 (analysis report §6.4): syscall_stub! macro to distinguish
// unimplemented syscalls (real ENOSYS) from planned stubs (to implement).
//
// Usage in table.rs:
//   syscall_stub!(sys_sendfile,   "sendfile not yet implemented");
//   syscall_stub!(sys_splice,     "splice not yet implemented");
//
// Without this macro, all ENOSYS were indistinguishable in metrics
// and impossible to determine whether it was an oversight or architectural decision.
// ─────────────────────────────────────────────────────────────────────────────

/// Generates a syscall handler that returns ENOSYS with a documented comment.
/// The comment appears in cargo doc and in ENOSYS audit reports.
#[macro_export]
macro_rules! syscall_stub {
    ($name:ident, $reason:expr) => {
        #[allow(non_snake_case)]
        pub fn $name(_a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
            // STUB: $reason
            // This syscall is intentionally unimplemented in v0.2.0.
            // Use syscall_stub! to document why (vs silent).
            $crate::syscall::numbers::ENOSYS
        }
    };
}
