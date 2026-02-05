// libs/exo_types/src/syscall.rs
//! System call numbers for Exo-OS

/// System call numbers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    
    // Memory management (60-79)
    Mmap = 60,
    Munmap = 61,
    Mprotect = 62,
    Madvise = 63,
    Brk = 64,
    Sbrk = 65,
    
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
    
    // Time (140-149)
    GetTime = 140,
    SetTime = 141,
    Nanosleep = 142,
    ClockGettime = 143,
    
    // Signals (150-159)
    Signal = 150,
    SigAction = 151,
    SigReturn = 152,
    SigProcMask = 153,
    
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
    
    // Capabilities (180-189)
    CapCreate = 180,
    CapRevoke = 181,
    CapGrant = 182,
    CapCheck = 183,
    
    // Debugging (190-199)
    DebugPrint = 190,
    DebugLog = 191,
    
    // System info (200-209)
    Uname = 200,
    GetRandom = 201,
    Sysinfo = 202,
    
    // Custom/Reserved (1000+)
    Custom = 1000,
}

impl SyscallNumber {
    /// Convert to raw syscall number
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self as u64
    }
    
    /// Convert from raw syscall number
    pub const fn from_u64(num: u64) -> Option<Self> {
        match num {
            0 => Some(Self::Exit),
            1 => Some(Self::Fork),
            2 => Some(Self::Exec),
            3 => Some(Self::Wait),
            4 => Some(Self::GetPid),
            20 => Some(Self::Open),
            21 => Some(Self::Close),
            22 => Some(Self::Read),
            23 => Some(Self::Write),
            60 => Some(Self::Mmap),
            61 => Some(Self::Munmap),
            80 => Some(Self::IpcSend),
            81 => Some(Self::IpcRecv),
            140 => Some(Self::GetTime),
            190 => Some(Self::DebugPrint),
            _ => None,
        }
    }
    
    /// Get syscall name
    pub const fn name(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::Fork => "fork",
            Self::Exec => "exec",
            Self::Wait => "wait",
            Self::GetPid => "getpid",
            Self::Open => "open",
            Self::Close => "close",
            Self::Read => "read",
            Self::Write => "write",
            Self::Mmap => "mmap",
            Self::Munmap => "munmap",
            Self::IpcSend => "ipc_send",
            Self::IpcRecv => "ipc_recv",
            Self::GetTime => "gettime",
            Self::DebugPrint => "debug_print",
            _ => "unknown",
        }
    }
}

/// Perform syscall with 0 arguments
#[inline]
pub unsafe fn syscall0(num: SyscallNumber) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

/// Perform syscall with 1 argument
#[inline]
pub unsafe fn syscall1(num: SyscallNumber, arg1: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

/// Perform syscall with 2 arguments
#[inline]
pub unsafe fn syscall2(num: SyscallNumber, arg1: usize, arg2: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        in("rdi") arg1,
        in("rsi") arg2,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

/// Perform syscall with 3 arguments
#[inline]
pub unsafe fn syscall3(num: SyscallNumber, arg1: usize, arg2: usize, arg3: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") num.as_u64(),
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

/// Perform syscall with 4 arguments
#[inline]
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
        options(nostack)
    );
    ret
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_syscall_numbers() {
        assert_eq!(SyscallNumber::Exit.as_u64(), 0);
        assert_eq!(SyscallNumber::Open.as_u64(), 20);
        assert_eq!(SyscallNumber::from_u64(1), Some(SyscallNumber::Fork));
    }
    
    #[test]
    fn test_syscall_names() {
        assert_eq!(SyscallNumber::Exit.name(), "exit");
        assert_eq!(SyscallNumber::Read.name(), "read");
    }
}
