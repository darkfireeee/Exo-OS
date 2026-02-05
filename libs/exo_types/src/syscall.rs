//! System call numbers and low-level syscall interface
//!
//! Provides type-safe syscall numbers and inline assembly wrappers
//! for x86-64 syscall interface following System V ABI.

use core::fmt;

/// System call number enumeration
///
/// Organized by category for maintainability. Each syscall has a unique
/// number that matches the kernel's syscall dispatch table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u64)]
pub enum SyscallNumber {
    // Process management (0-19)
    Exit = 0,
    Fork = 1,
    Exec = 2,
    Wait = 3,
    GetPid = 4,
    GetPPid = 5,
    Kill = 6,
    GetUid = 7,
    GetGid = 8,
    SetUid = 9,
    SetGid = 10,
    
    // File I/O (20-49)
    Open = 20,
    Close = 21,
    Read = 22,
    Write = 23,
    Seek = 24,
    Ioctl = 25,
    Dup = 26,
    Dup2 = 27,
    Pipe = 28,
    Stat = 29,
    Fstat = 30,
    Lstat = 31,
    Access = 32,
    Chmod = 33,
    Chown = 34,
    Truncate = 35,
    Ftruncate = 36,
    Sync = 37,
    Fsync = 38,
    
    // Directory operations (50-59)
    Chdir = 50,
    Getcwd = 51,
    Mkdir = 52,
    Rmdir = 53,
    Readdir = 54,
    Unlink = 55,
    Link = 56,
    Symlink = 57,
    Readlink = 58,
    Rename = 59,
    
    // Memory management (60-79)
    Mmap = 60,
    Munmap = 61,
    Mprotect = 62,
    Madvise = 63,
    Brk = 64,
    Sbrk = 65,
    Msync = 66,
    Mlock = 67,
    Munlock = 68,
    
    // IPC (80-99)
    IpcSend = 80,
    IpcRecv = 81,
    IpcCall = 82,
    IpcReply = 83,
    CreateChannel = 84,
    DestroyChannel = 85,
    Connect = 86,
    Accept = 87,
    
    // Threading (100-119)
    ThreadCreate = 100,
    ThreadExit = 101,
    ThreadJoin = 102,
    ThreadYield = 103,
    ThreadSleep = 104,
    GetTid = 105,
    SetThreadArea = 106,
    GetThreadArea = 107,
    
    // Synchronization (120-139)
    MutexCreate = 120,
    MutexLock = 121,
    MutexUnlock = 122,
    MutexDestroy = 123,
    CondCreate = 124,
    CondWait = 125,
    CondSignal = 126,
    CondDestroy = 127,
    SemCreate = 128,
    SemWait = 129,
    SemPost = 130,
    SemDestroy = 131,
    Futex = 132,
    
    // Time (140-149)
    GetTime = 140,
    SetTime = 141,
    Nanosleep = 142,
    ClockGettime = 143,
    ClockSettime = 144,
    ClockNanosleep = 145,
    
    // Signals (150-159)
    Signal = 150,
    SigAction = 151,
    SigReturn = 152,
    SigProcMask = 153,
    SigPending = 154,
    SigSuspend = 155,
    
    // Networking (160-179)
    Socket = 160,
    Bind = 161,
    Listen = 162,
    NetAccept = 163,
    NetConnect = 164,
    Send = 165,
    Recv = 166,
    Sendto = 167,
    Recvfrom = 168,
    Shutdown = 169,
    GetSockOpt = 170,
    SetSockOpt = 171,
    GetPeerName = 172,
    GetSockName = 173,
    
    // Capabilities (180-189)
    CapCreate = 180,
    CapRevoke = 181,
    CapGrant = 182,
    CapCheck = 183,
    CapDerive = 184,
    
    // Debugging (190-199)
    DebugPrint = 190,
    DebugLog = 191,
    DebugBreak = 192,
    
    // System info (200-209)
    Uname = 200,
    GetRandom = 201,
    Sysinfo = 202,
    Sysconf = 203,
    GetRusage = 204,
    
    // Custom/Reserved (1000+)
    Custom = 1000,
}

impl SyscallNumber {
    /// Convert to raw syscall number
    #[inline(always)]
    pub const fn as_u64(self) -> u64 {
        self as u64
    }
    
    /// Convert to usize for register operations
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self as usize
    }
    
    /// Convert from raw syscall number
    #[inline(always)]
    pub const fn from_u64(num: u64) -> Option<Self> {
        match num {
            // Process management
            0 => Some(Self::Exit),
            1 => Some(Self::Fork),
            2 => Some(Self::Exec),
            3 => Some(Self::Wait),
            4 => Some(Self::GetPid),
            5 => Some(Self::GetPPid),
            6 => Some(Self::Kill),
            7 => Some(Self::GetUid),
            8 => Some(Self::GetGid),
            9 => Some(Self::SetUid),
            10 => Some(Self::SetGid),
            
            // File I/O
            20 => Some(Self::Open),
            21 => Some(Self::Close),
            22 => Some(Self::Read),
            23 => Some(Self::Write),
            24 => Some(Self::Seek),
            25 => Some(Self::Ioctl),
            26 => Some(Self::Dup),
            27 => Some(Self::Dup2),
            28 => Some(Self::Pipe),
            29 => Some(Self::Stat),
            30 => Some(Self::Fstat),
            31 => Some(Self::Lstat),
            32 => Some(Self::Access),
            33 => Some(Self::Chmod),
            34 => Some(Self::Chown),
            35 => Some(Self::Truncate),
            36 => Some(Self::Ftruncate),
            37 => Some(Self::Sync),
            38 => Some(Self::Fsync),
            
            // Directory operations
            50 => Some(Self::Chdir),
            51 => Some(Self::Getcwd),
            52 => Some(Self::Mkdir),
            53 => Some(Self::Rmdir),
            54 => Some(Self::Readdir),
            55 => Some(Self::Unlink),
            56 => Some(Self::Link),
            57 => Some(Self::Symlink),
            58 => Some(Self::Readlink),
            59 => Some(Self::Rename),
            
            // Memory management
            60 => Some(Self::Mmap),
            61 => Some(Self::Munmap),
            62 => Some(Self::Mprotect),
            63 => Some(Self::Madvise),
            64 => Some(Self::Brk),
            65 => Some(Self::Sbrk),
            66 => Some(Self::Msync),
            67 => Some(Self::Mlock),
            68 => Some(Self::Munlock),
            
            // IPC
            80 => Some(Self::IpcSend),
            81 => Some(Self::IpcRecv),
            82 => Some(Self::IpcCall),
            83 => Some(Self::IpcReply),
            84 => Some(Self::CreateChannel),
            85 => Some(Self::DestroyChannel),
            86 => Some(Self::Connect),
            87 => Some(Self::Accept),
            
            // Threading
            100 => Some(Self::ThreadCreate),
            101 => Some(Self::ThreadExit),
            102 => Some(Self::ThreadJoin),
            103 => Some(Self::ThreadYield),
            104 => Some(Self::ThreadSleep),
            105 => Some(Self::GetTid),
            106 => Some(Self::SetThreadArea),
            107 => Some(Self::GetThreadArea),
            
            // Synchronization
            120 => Some(Self::MutexCreate),
            121 => Some(Self::MutexLock),
            122 => Some(Self::MutexUnlock),
            123 => Some(Self::MutexDestroy),
            124 => Some(Self::CondCreate),
            125 => Some(Self::CondWait),
            126 => Some(Self::CondSignal),
            127 => Some(Self::CondDestroy),
            128 => Some(Self::SemCreate),
            129 => Some(Self::SemWait),
            130 => Some(Self::SemPost),
            131 => Some(Self::SemDestroy),
            132 => Some(Self::Futex),
            
            // Time
            140 => Some(Self::GetTime),
            141 => Some(Self::SetTime),
            142 => Some(Self::Nanosleep),
            143 => Some(Self::ClockGettime),
            144 => Some(Self::ClockSettime),
            145 => Some(Self::ClockNanosleep),
            
            // Signals
            150 => Some(Self::Signal),
            151 => Some(Self::SigAction),
            152 => Some(Self::SigReturn),
            153 => Some(Self::SigProcMask),
            154 => Some(Self::SigPending),
            155 => Some(Self::SigSuspend),
            
            // Networking
            160 => Some(Self::Socket),
            161 => Some(Self::Bind),
            162 => Some(Self::Listen),
            163 => Some(Self::NetAccept),
            164 => Some(Self::NetConnect),
            165 => Some(Self::Send),
            166 => Some(Self::Recv),
            167 => Some(Self::Sendto),
            168 => Some(Self::Recvfrom),
            169 => Some(Self::Shutdown),
            170 => Some(Self::GetSockOpt),
            171 => Some(Self::SetSockOpt),
            172 => Some(Self::GetPeerName),
            173 => Some(Self::GetSockName),
            
            // Capabilities
            180 => Some(Self::CapCreate),
            181 => Some(Self::CapRevoke),
            182 => Some(Self::CapGrant),
            183 => Some(Self::CapCheck),
            184 => Some(Self::CapDerive),
            
            // Debugging
            190 => Some(Self::DebugPrint),
            191 => Some(Self::DebugLog),
            192 => Some(Self::DebugBreak),
            
            // System info
            200 => Some(Self::Uname),
            201 => Some(Self::GetRandom),
            202 => Some(Self::Sysinfo),
            203 => Some(Self::Sysconf),
            204 => Some(Self::GetRusage),
            
            // Custom
            1000 => Some(Self::Custom),
            
            _ => None,
        }
    }
    
    /// Get syscall name as string
    #[inline(always)]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::Fork => "fork",
            Self::Exec => "exec",
            Self::Wait => "wait",
            Self::GetPid => "getpid",
            Self::GetPPid => "getppid",
            Self::Kill => "kill",
            Self::GetUid => "getuid",
            Self::GetGid => "getgid",
            Self::SetUid => "setuid",
            Self::SetGid => "setgid",
            
            Self::Open => "open",
            Self::Close => "close",
            Self::Read => "read",
            Self::Write => "write",
            Self::Seek => "seek",
            Self::Ioctl => "ioctl",
            Self::Dup => "dup",
            Self::Dup2 => "dup2",
            Self::Pipe => "pipe",
            Self::Stat => "stat",
            Self::Fstat => "fstat",
            Self::Lstat => "lstat",
            Self::Access => "access",
            Self::Chmod => "chmod",
            Self::Chown => "chown",
            Self::Truncate => "truncate",
            Self::Ftruncate => "ftruncate",
            Self::Sync => "sync",
            Self::Fsync => "fsync",
            
            Self::Chdir => "chdir",
            Self::Getcwd => "getcwd",
            Self::Mkdir => "mkdir",
            Self::Rmdir => "rmdir",
            Self::Readdir => "readdir",
            Self::Unlink => "unlink",
            Self::Link => "link",
            Self::Symlink => "symlink",
            Self::Readlink => "readlink",
            Self::Rename => "rename",
            
            Self::Mmap => "mmap",
            Self::Munmap => "munmap",
            Self::Mprotect => "mprotect",
            Self::Madvise => "madvise",
            Self::Brk => "brk",
            Self::Sbrk => "sbrk",
            Self::Msync => "msync",
            Self::Mlock => "mlock",
            Self::Munlock => "munlock",
            
            Self::IpcSend => "ipc_send",
            Self::IpcRecv => "ipc_recv",
            Self::IpcCall => "ipc_call",
            Self::IpcReply => "ipc_reply",
            Self::CreateChannel => "create_channel",
            Self::DestroyChannel => "destroy_channel",
            Self::Connect => "connect",
            Self::Accept => "accept",
            
            Self::ThreadCreate => "thread_create",
            Self::ThreadExit => "thread_exit",
            Self::ThreadJoin => "thread_join",
            Self::ThreadYield => "thread_yield",
            Self::ThreadSleep => "thread_sleep",
            Self::GetTid => "gettid",
            Self::SetThreadArea => "set_thread_area",
            Self::GetThreadArea => "get_thread_area",
            
            Self::MutexCreate => "mutex_create",
            Self::MutexLock => "mutex_lock",
            Self::MutexUnlock => "mutex_unlock",
            Self::MutexDestroy => "mutex_destroy",
            Self::CondCreate => "cond_create",
            Self::CondWait => "cond_wait",
            Self::CondSignal => "cond_signal",
            Self::CondDestroy => "cond_destroy",
            Self::SemCreate => "sem_create",
            Self::SemWait => "sem_wait",
            Self::SemPost => "sem_post",
            Self::SemDestroy => "sem_destroy",
            Self::Futex => "futex",
            
            Self::GetTime => "gettime",
            Self::SetTime => "settime",
            Self::Nanosleep => "nanosleep",
            Self::ClockGettime => "clock_gettime",
            Self::ClockSettime => "clock_settime",
            Self::ClockNanosleep => "clock_nanosleep",
            
            Self::Signal => "signal",
            Self::SigAction => "sigaction",
            Self::SigReturn => "sigreturn",
            Self::SigProcMask => "sigprocmask",
            Self::SigPending => "sigpending",
            Self::SigSuspend => "sigsuspend",
            
            Self::Socket => "socket",
            Self::Bind => "bind",
            Self::Listen => "listen",
            Self::NetAccept => "net_accept",
            Self::NetConnect => "net_connect",
            Self::Send => "send",
            Self::Recv => "recv",
            Self::Sendto => "sendto",
            Self::Recvfrom => "recvfrom",
            Self::Shutdown => "shutdown",
            Self::GetSockOpt => "getsockopt",
            Self::SetSockOpt => "setsockopt",
            Self::GetPeerName => "getpeername",
            Self::GetSockName => "getsockname",
            
            Self::CapCreate => "cap_create",
            Self::CapRevoke => "cap_revoke",
            Self::CapGrant => "cap_grant",
            Self::CapCheck => "cap_check",
            Self::CapDerive => "cap_derive",
            
            Self::DebugPrint => "debug_print",
            Self::DebugLog => "debug_log",
            Self::DebugBreak => "debug_break",
            
            Self::Uname => "uname",
            Self::GetRandom => "getrandom",
            Self::Sysinfo => "sysinfo",
            Self::Sysconf => "sysconf",
            Self::GetRusage => "getrusage",
            
            Self::Custom => "custom",
        }
    }
}

impl fmt::Display for SyscallNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", self.name(), self.as_u64())
    }
}

/// Perform syscall with 0 arguments
///
/// # Safety
/// Caller must ensure syscall number and semantics are correct.
/// Syscall may modify kernel state.
#[inline(always)]
pub unsafe fn syscall0(num: SyscallNumber) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

/// Perform syscall with 1 argument
#[inline(always)]
pub unsafe fn syscall1(num: SyscallNumber, arg1: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        in("rdi") arg1,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

/// Perform syscall with 2 arguments
#[inline(always)]
pub unsafe fn syscall2(num: SyscallNumber, arg1: usize, arg2: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        in("rdi") arg1,
        in("rsi") arg2,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

/// Perform syscall with 3 arguments
#[inline(always)]
pub unsafe fn syscall3(num: SyscallNumber, arg1: usize, arg2: usize, arg3: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

/// Perform syscall with 4 arguments
#[inline(always)]
pub unsafe fn syscall4(num: SyscallNumber, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        in("r10") arg4,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

/// Perform syscall with 5 arguments
#[inline(always)]
pub unsafe fn syscall5(num: SyscallNumber, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        in("r10") arg4,
        in("r8") arg5,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

/// Perform syscall with 6 arguments
#[inline(always)]
pub unsafe fn syscall6(num: SyscallNumber, arg1: usize, arg2: usize, arg3: usize, arg4: usize, arg5: usize, arg6: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        in("r10") arg4,
        in("r8") arg5,
        in("r9") arg6,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    extern crate std;
    use std::mem::size_of;
    
    #[test]
    fn test_syscall_numbers() {
        assert_eq!(SyscallNumber::Exit.as_u64(), 0);
        assert_eq!(SyscallNumber::Fork.as_u64(), 1);
        assert_eq!(SyscallNumber::Open.as_u64(), 20);
        assert_eq!(SyscallNumber::Mmap.as_u64(), 60);
    }
    
    #[test]
    fn test_syscall_from_u64() {
        assert_eq!(SyscallNumber::from_u64(0), Some(SyscallNumber::Exit));
        assert_eq!(SyscallNumber::from_u64(1), Some(SyscallNumber::Fork));
        assert_eq!(SyscallNumber::from_u64(20), Some(SyscallNumber::Open));
        assert_eq!(SyscallNumber::from_u64(9999), None);
    }
    
    #[test]
    fn test_syscall_from_u64_complete() {
        // Test all defined syscalls have round-trip
        for i in 0..=1000 {
            if let Some(syscall) = SyscallNumber::from_u64(i) {
                assert_eq!(syscall.as_u64(), i);
            }
        }
    }
    
    #[test]
    fn test_syscall_names() {
        assert_eq!(SyscallNumber::Exit.name(), "exit");
        assert_eq!(SyscallNumber::Fork.name(), "fork");
        assert_eq!(SyscallNumber::Read.name(), "read");
        assert_eq!(SyscallNumber::Write.name(), "write");
    }
    
    #[test]
    fn test_syscall_display() {
        let s = std::format!("{}", SyscallNumber::Exit);
        assert_eq!(s, "exit(0)");
        
        let s = std::format!("{}", SyscallNumber::Read);
        assert_eq!(s, "read(22)");
    }
    
    #[test]
    fn test_syscall_ordering() {
        assert!(SyscallNumber::Exit < SyscallNumber::Fork);
        assert!(SyscallNumber::Open < SyscallNumber::Read);
    }
    
    #[test]
    fn test_syscall_size() {
        assert_eq!(size_of::<SyscallNumber>(), size_of::<u64>());
    }
    
    #[test]
    fn test_syscall_copy() {
        let s1 = SyscallNumber::Exit;
        let s2 = s1;
        assert_eq!(s1, s2);
    }
}
