//! System call numbers
//!
//! Enumeration of all system calls in Exo-OS.
//! Based on Linux syscall numbers with Exo-OS extensions.

use core::fmt;

/// System call number enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum SyscallNumber {
    // ===== Process Management =====
    /// Read from file descriptor
    Read = 0,
    /// Write to file descriptor
    Write = 1,
    /// Open file
    Open = 2,
    /// Close file descriptor
    Close = 3,
    /// Get file status
    Stat = 4,
    /// Get file status (file descriptor)
    Fstat = 5,
    /// Get file status (no follow symlinks)
    Lstat = 6,
    /// Poll file descriptors
    Poll = 7,
    /// Reposition file offset
    Lseek = 8,
    /// Map files or devices into memory
    Mmap = 9,
    /// Set memory protection
    Mprotect = 10,
    /// Unmap files or devices from memory
    Munmap = 11,
    /// Program break (heap management)
    Brk = 12,
    /// Signal action
    RtSigaction = 13,
    /// Signal mask
    RtSigprocmask = 14,
    /// Signal return
    RtSigreturn = 15,
    /// I/O control
    Ioctl = 16,
    /// Read vector
    Pread64 = 17,
    /// Write vector
    Pwrite64 = 18,
    /// Read vector
    Readv = 19,
    /// Write vector
    Writev = 20,

    // ===== File Operations =====
    /// Access check
    Access = 21,
    /// Create pipe
    Pipe = 22,
    /// Select
    Select = 23,
    /// Schedule yield
    SchedYield = 24,
    /// Remap pages
    Mremap = 25,
    /// Sync memory
    Msync = 26,
    /// Get minimum core size
    Mincore = 27,
    /// Advise memory usage
    Madvise = 28,
    /// Shared memory control
    Shmget = 29,
    /// Shared memory attach
    Shmat = 30,
    /// Shared memory control
    Shmctl = 31,
    /// Duplicate file descriptor
    Dup = 32,
    /// Duplicate file descriptor (with specific number)
    Dup2 = 33,
    /// Pause
    Pause = 34,
    /// Nanosleep
    Nanosleep = 35,
    /// Get interval timer
    Getitimer = 36,
    /// Alarm
    Alarm = 37,
    /// Set interval timer
    Setitimer = 38,
    /// Get process ID
    Getpid = 39,

    // ===== Network Operations =====
    /// Send file
    Sendfile = 40,
    /// Create socket
    Socket = 41,
    /// Connect socket
    Connect = 42,
    /// Accept connection
    Accept = 43,
    /// Send message
    Sendto = 44,
    /// Receive message
    Recvfrom = 45,
    /// Send message
    Sendmsg = 46,
    /// Receive message
    Recvmsg = 47,
    /// Shutdown socket
    Shutdown = 48,
    /// Bind socket
    Bind = 49,
    /// Listen on socket
    Listen = 50,
    /// Get socket name
    Getsockname = 51,
    /// Get peer name
    Getpeername = 52,
    /// Create socket pair
    Socketpair = 53,
    /// Set socket options
    Setsockopt = 54,
    /// Get socket options
    Getsockopt = 55,

    // ===== Process Control =====
    /// Clone process
    Clone = 56,
    /// Fork process
    Fork = 57,
    /// Fork process (BSD)
    Vfork = 58,
    /// Execute program
    Execve = 59,
    /// Exit process
    Exit = 60,
    /// Wait for process
    Wait4 = 61,
    /// Send signal
    Kill = 62,
    /// Get system information
    Uname = 63,
    /// Semaphore get
    Semget = 64,
    /// Semaphore operation
    Semop = 65,
    /// Semaphore control
    Semctl = 66,
    /// Message get
    Msgget = 68,
    /// Message send
    Msgsnd = 69,
    /// Message receive
    Msgrcv = 70,
    /// Message control
    Msgctl = 71,

    // ===== File Control =====
    /// File control
    Fcntl = 72,
    /// File lock
    Flock = 73,
    /// Sync file
    Fsync = 74,
    /// Sync file data
    Fdatasync = 75,
    /// Truncate file
    Truncate = 76,
    /// Truncate file (file descriptor)
    Ftruncate = 77,
    /// Get directory entries
    Getdents = 78,
    /// Get current working directory
    Getcwd = 79,
    /// Change directory
    Chdir = 80,
    /// Change directory (file descriptor)
    Fchdir = 81,
    /// Rename file
    Rename = 82,
    /// Create directory
    Mkdir = 83,
    /// Remove directory
    Rmdir = 84,
    /// Create file
    Creat = 85,
    /// Link file
    Link = 86,
    /// Unlink file
    Unlink = 87,
    /// Create symbolic link
    Symlink = 88,
    /// Read symbolic link
    Readlink = 89,
    /// Change file mode
    Chmod = 90,
    /// Change file mode (file descriptor)
    Fchmod = 91,
    /// Change file owner
    Chown = 92,
    /// Change file owner (file descriptor)
    Fchown = 93,
    /// Change file owner (no follow symlinks)
    Lchown = 94,
    /// Set user mask
    Umask = 95,

    // ===== Time Operations =====
    /// Get time of day
    Gettimeofday = 96,
    /// Get resource limits
    Getrlimit = 97,
    /// Get resource usage
    Getrusage = 98,
    /// Get system information
    Sysinfo = 99,
    /// Get time
    Times = 100,

    // ===== IPC & Debugging =====
    /// Process trace
    Ptrace = 101,
    /// Get user ID
    Getuid = 102,
    /// System log
    Syslog = 103,
    /// Get group ID
    Getgid = 104,
    /// Set user ID
    Setuid = 105,
    /// Set group ID
    Setgid = 106,
    /// Get effective user ID
    Geteuid = 107,
    /// Get effective group ID
    Getegid = 108,
    /// Set process group ID
    Setpgid = 109,
    /// Get parent process ID
    Getppid = 110,
    /// Get process group
    Getpgrp = 111,
    /// Create session
    Setsid = 112,
    /// Set real and effective user IDs
    Setreuid = 113,
    /// Set real and effective group IDs
    Setregid = 114,
    /// Get groups
    Getgroups = 115,
    /// Set groups
    Setgroups = 116,
    /// Set real, effective and saved user ID
    Setresuid = 117,
    /// Get real, effective and saved user ID
    Getresuid = 118,
    /// Set real, effective and saved group ID
    Setresgid = 119,
    /// Get real, effective and saved group ID
    Getresgid = 120,

    // ===== Exo-OS Custom Syscalls (1000+) =====
    /// Create capability
    CapCreate = 1000,
    /// Transfer capability
    CapTransfer = 1001,
    /// Revoke capability
    CapRevoke = 1002,
    /// Query capability
    CapQuery = 1003,
    /// Microkernel IPC send
    IpcSend = 1010,
    /// Microkernel IPC receive
    IpcRecv = 1011,
    /// Microkernel IPC call (send+receive)
    IpcCall = 1012,
    /// Query system information
    ExoInfo = 1020,
    /// Set security policy
    SetSecPolicy = 1021,
    /// Get security policy
    GetSecPolicy = 1022,
}

impl SyscallNumber {
    /// Convert syscall number to raw usize value
    #[inline(always)]
    pub const fn as_raw(self) -> usize {
        self as usize
    }

    /// Convert raw usize to syscall number (returns None if invalid)
    #[inline]
    pub const fn from_raw(num: usize) -> Option<Self> {
        match num {
            0 => Some(Self::Read),
            1 => Some(Self::Write),
            2 => Some(Self::Open),
            3 => Some(Self::Close),
            4 => Some(Self::Stat),
            5 => Some(Self::Fstat),
            6 => Some(Self::Lstat),
            7 => Some(Self::Poll),
            8 => Some(Self::Lseek),
            9 => Some(Self::Mmap),
            10 => Some(Self::Mprotect),
            11 => Some(Self::Munmap),
            12 => Some(Self::Brk),
            13 => Some(Self::RtSigaction),
            14 => Some(Self::RtSigprocmask),
            15 => Some(Self::RtSigreturn),
            16 => Some(Self::Ioctl),
            17 => Some(Self::Pread64),
            18 => Some(Self::Pwrite64),
            19 => Some(Self::Readv),
            20 => Some(Self::Writev),
            39 => Some(Self::Getpid),
            60 => Some(Self::Exit),
            // Add more mappings as needed
            1000 => Some(Self::CapCreate),
            1001 => Some(Self::CapTransfer),
            1002 => Some(Self::CapRevoke),
            1003 => Some(Self::CapQuery),
            1010 => Some(Self::IpcSend),
            1011 => Some(Self::IpcRecv),
            1012 => Some(Self::IpcCall),
            1020 => Some(Self::ExoInfo),
            1021 => Some(Self::SetSecPolicy),
            1022 => Some(Self::GetSecPolicy),
            _ => None,
        }
    }

    /// Get syscall name as string
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Open => "open",
            Self::Close => "close",
            Self::Stat => "stat",
            Self::Fstat => "fstat",
            Self::Lstat => "lstat",
            Self::Poll => "poll",
            Self::Lseek => "lseek",
            Self::Mmap => "mmap",
            Self::Mprotect => "mprotect",
            Self::Munmap => "munmap",
            Self::Brk => "brk",
            Self::RtSigaction => "rt_sigaction",
            Self::RtSigprocmask => "rt_sigprocmask",
            Self::RtSigreturn => "rt_sigreturn",
            Self::Ioctl => "ioctl",
            Self::Pread64 => "pread64",
            Self::Pwrite64 => "pwrite64",
            Self::Readv => "readv",
            Self::Writev => "writev",
            Self::Access => "access",
            Self::Pipe => "pipe",
            Self::Select => "select",
            Self::SchedYield => "sched_yield",
            Self::Mremap => "mremap",
            Self::Msync => "msync",
            Self::Mincore => "mincore",
            Self::Madvise => "madvise",
            Self::Shmget => "shmget",
            Self::Shmat => "shmat",
            Self::Shmctl => "shmctl",
            Self::Dup => "dup",
            Self::Dup2 => "dup2",
            Self::Pause => "pause",
            Self::Nanosleep => "nanosleep",
            Self::Getitimer => "getitimer",
            Self::Alarm => "alarm",
            Self::Setitimer => "setitimer",
            Self::Getpid => "getpid",
            Self::Sendfile => "sendfile",
            Self::Socket => "socket",
            Self::Connect => "connect",
            Self::Accept => "accept",
            Self::Sendto => "sendto",
            Self::Recvfrom => "recvfrom",
            Self::Sendmsg => "sendmsg",
            Self::Recvmsg => "recvmsg",
            Self::Shutdown => "shutdown",
            Self::Bind => "bind",
            Self::Listen => "listen",
            Self::Getsockname => "getsockname",
            Self::Getpeername => "getpeername",
            Self::Socketpair => "socketpair",
            Self::Setsockopt => "setsockopt",
            Self::Getsockopt => "getsockopt",
            Self::Clone => "clone",
            Self::Fork => "fork",
            Self::Vfork => "vfork",
            Self::Execve => "execve",
            Self::Exit => "exit",
            Self::Wait4 => "wait4",
            Self::Kill => "kill",
            Self::Uname => "uname",
            Self::Semget => "semget",
            Self::Semop => "semop",
            Self::Semctl => "semctl",
            Self::Msgget => "msgget",
            Self::Msgsnd => "msgsnd",
            Self::Msgrcv => "msgrcv",
            Self::Msgctl => "msgctl",
            Self::Fcntl => "fcntl",
            Self::Flock => "flock",
            Self::Fsync => "fsync",
            Self::Fdatasync => "fdatasync",
            Self::Truncate => "truncate",
            Self::Ftruncate => "ftruncate",
            Self::Getdents => "getdents",
            Self::Getcwd => "getcwd",
            Self::Chdir => "chdir",
            Self::Fchdir => "fchdir",
            Self::Rename => "rename",
            Self::Mkdir => "mkdir",
            Self::Rmdir => "rmdir",
            Self::Creat => "creat",
            Self::Link => "link",
            Self::Unlink => "unlink",
            Self::Symlink => "symlink",
            Self::Readlink => "readlink",
            Self::Chmod => "chmod",
            Self::Fchmod => "fchmod",
            Self::Chown => "chown",
            Self::Fchown => "fchown",
            Self::Lchown => "lchown",
            Self::Umask => "umask",
            Self::Gettimeofday => "gettimeofday",
            Self::Getrlimit => "getrlimit",
            Self::Getrusage => "getrusage",
            Self::Sysinfo => "sysinfo",
            Self::Times => "times",
            Self::Ptrace => "ptrace",
            Self::Getuid => "getuid",
            Self::Syslog => "syslog",
            Self::Getgid => "getgid",
            Self::Setuid => "setuid",
            Self::Setgid => "setgid",
            Self::Geteuid => "geteuid",
            Self::Getegid => "getegid",
            Self::Setpgid => "setpgid",
            Self::Getppid => "getppid",
            Self::Getpgrp => "getpgrp",
            Self::Setsid => "setsid",
            Self::Setreuid => "setreuid",
            Self::Setregid => "setregid",
            Self::Getgroups => "getgroups",
            Self::Setgroups => "setgroups",
            Self::Setresuid => "setresuid",
            Self::Getresuid => "getresuid",
            Self::Setresgid => "setresgid",
            Self::Getresgid => "getresgid",
            Self::CapCreate => "cap_create",
            Self::CapTransfer => "cap_transfer",
            Self::CapRevoke => "cap_revoke",
            Self::CapQuery => "cap_query",
            Self::IpcSend => "ipc_send",
            Self::IpcRecv => "ipc_recv",
            Self::IpcCall => "ipc_call",
            Self::ExoInfo => "exo_info",
            Self::SetSecPolicy => "set_sec_policy",
            Self::GetSecPolicy => "get_sec_policy",
        }
    }

    /// Check if this is an Exo-OS custom syscall
    #[inline(always)]
    pub const fn is_exo_syscall(self) -> bool {
        (self as usize) >= 1000
    }

    /// Check if this is a POSIX standard syscall
    #[inline(always)]
    pub const fn is_posix_syscall(self) -> bool {
        !self.is_exo_syscall()
    }
}

impl fmt::Display for SyscallNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.as_str(), self.as_raw())
    }
}

impl From<SyscallNumber> for usize {
    #[inline(always)]
    fn from(syscall: SyscallNumber) -> usize {
        syscall.as_raw()
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;

    #[test]
    fn test_syscall_conversions() {
        assert_eq!(SyscallNumber::Read.as_raw(), 0);
        assert_eq!(SyscallNumber::Write.as_raw(), 1);
        assert_eq!(SyscallNumber::CapCreate.as_raw(), 1000);

        assert_eq!(SyscallNumber::from_raw(0), Some(SyscallNumber::Read));
        assert_eq!(SyscallNumber::from_raw(1), Some(SyscallNumber::Write));
        assert_eq!(SyscallNumber::from_raw(1000), Some(SyscallNumber::CapCreate));
    }

    #[test]
    fn test_syscall_as_str() {
        assert_eq!(SyscallNumber::Read.as_str(), "read");
        assert_eq!(SyscallNumber::Write.as_str(), "write");
        assert_eq!(SyscallNumber::CapCreate.as_str(), "cap_create");
    }

    #[test]
    fn test_syscall_is_exo() {
        assert!(!SyscallNumber::Read.is_exo_syscall());
        assert!(!SyscallNumber::Write.is_exo_syscall());
        assert!(SyscallNumber::CapCreate.is_exo_syscall());
        assert!(SyscallNumber::IpcSend.is_exo_syscall());
    }

    #[test]
    fn test_syscall_is_posix() {
        assert!(SyscallNumber::Read.is_posix_syscall());
        assert!(SyscallNumber::Write.is_posix_syscall());
        assert!(!SyscallNumber::CapCreate.is_posix_syscall());
        assert!(!SyscallNumber::IpcSend.is_posix_syscall());
    }
}
