//! # Syscall Translation - POSIX to Exo-OS Native
//!
//! Translates POSIX syscalls to Exo-OS native calls using the optimal path.
//!
//! ## Execution Paths
//!
//! - **Fast Path** (70%): Direct mapping, < 50 cycles
//!   - getpid, getuid, clock_gettime (vDSO), etc.
//! - **Hybrid Path** (25%): Optimized translation, 400-1000 cycles
//!   - open, read, write, close, mmap, etc.
//! - **Legacy Path** (5%): Full emulation, 8000-50000 cycles
//!   - fork, ptrace, etc.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{ExecutionPath, PosixXError};

/// Syscall numbers (x86_64 Linux ABI)
pub mod syscall_numbers {
    pub const SYS_READ: i64 = 0;
    pub const SYS_WRITE: i64 = 1;
    pub const SYS_OPEN: i64 = 2;
    pub const SYS_CLOSE: i64 = 3;
    pub const SYS_STAT: i64 = 4;
    pub const SYS_FSTAT: i64 = 5;
    pub const SYS_LSTAT: i64 = 6;
    pub const SYS_POLL: i64 = 7;
    pub const SYS_LSEEK: i64 = 8;
    pub const SYS_MMAP: i64 = 9;
    pub const SYS_MPROTECT: i64 = 10;
    pub const SYS_MUNMAP: i64 = 11;
    pub const SYS_BRK: i64 = 12;
    pub const SYS_RT_SIGACTION: i64 = 13;
    pub const SYS_RT_SIGPROCMASK: i64 = 14;
    pub const SYS_RT_SIGRETURN: i64 = 15;
    pub const SYS_IOCTL: i64 = 16;
    pub const SYS_PREAD64: i64 = 17;
    pub const SYS_PWRITE64: i64 = 18;
    pub const SYS_READV: i64 = 19;
    pub const SYS_WRITEV: i64 = 20;
    pub const SYS_PIPE: i64 = 22;
    pub const SYS_SELECT: i64 = 23;
    pub const SYS_DUP: i64 = 32;
    pub const SYS_DUP2: i64 = 33;
    pub const SYS_GETPID: i64 = 39;
    pub const SYS_SOCKET: i64 = 41;
    pub const SYS_CONNECT: i64 = 42;
    pub const SYS_ACCEPT: i64 = 43;
    pub const SYS_SENDTO: i64 = 44;
    pub const SYS_RECVFROM: i64 = 45;
    pub const SYS_SENDMSG: i64 = 46;
    pub const SYS_RECVMSG: i64 = 47;
    pub const SYS_SHUTDOWN: i64 = 48;
    pub const SYS_BIND: i64 = 49;
    pub const SYS_LISTEN: i64 = 50;
    pub const SYS_CLONE: i64 = 56;
    pub const SYS_FORK: i64 = 57;
    pub const SYS_VFORK: i64 = 58;
    pub const SYS_EXECVE: i64 = 59;
    pub const SYS_EXIT: i64 = 60;
    pub const SYS_WAIT4: i64 = 61;
    pub const SYS_KILL: i64 = 62;
    pub const SYS_FCNTL: i64 = 72;
    pub const SYS_FLOCK: i64 = 73;
    pub const SYS_FSYNC: i64 = 74;
    pub const SYS_GETUID: i64 = 102;
    pub const SYS_GETGID: i64 = 104;
    pub const SYS_GETEUID: i64 = 107;
    pub const SYS_GETEGID: i64 = 108;
    pub const SYS_GETPPID: i64 = 110;
    pub const SYS_CLOCK_GETTIME: i64 = 228;
    pub const SYS_CLOCK_GETRES: i64 = 229;
    pub const SYS_CLOCK_NANOSLEEP: i64 = 230;
    pub const SYS_EPOLL_CREATE: i64 = 213;
    pub const SYS_EPOLL_CTL: i64 = 233;
    pub const SYS_EPOLL_WAIT: i64 = 232;
}

use syscall_numbers::*;

/// Syscall translator
pub struct SyscallTranslator {
    /// Translation statistics
    fast_count: u64,
    hybrid_count: u64,
    legacy_count: u64,
}

impl SyscallTranslator {
    /// Create new translator
    pub fn new() -> Self {
        Self {
            fast_count: 0,
            hybrid_count: 0,
            legacy_count: 0,
        }
    }

    /// Determine execution path for syscall
    pub fn classify_syscall(&self, syscall_num: i64) -> ExecutionPath {
        match syscall_num {
            // Fast path - simple getters (< 50 cycles)
            SYS_GETPID | SYS_GETUID | SYS_GETGID | SYS_GETEUID | SYS_GETEGID | SYS_GETPPID => {
                ExecutionPath::Fast
            }
            SYS_CLOCK_GETTIME | SYS_CLOCK_GETRES => ExecutionPath::Fast,
            SYS_BRK => ExecutionPath::Fast,

            // Hybrid path - file operations (400-1000 cycles)
            SYS_OPEN | SYS_CLOSE | SYS_READ | SYS_WRITE | SYS_LSEEK => ExecutionPath::Hybrid,
            SYS_STAT | SYS_FSTAT | SYS_LSTAT => ExecutionPath::Hybrid,
            SYS_MMAP | SYS_MUNMAP | SYS_MPROTECT => ExecutionPath::Hybrid,
            SYS_PIPE | SYS_DUP | SYS_DUP2 => ExecutionPath::Hybrid,
            SYS_POLL | SYS_SELECT | SYS_EPOLL_CREATE | SYS_EPOLL_CTL | SYS_EPOLL_WAIT => {
                ExecutionPath::Hybrid
            }
            SYS_SOCKET | SYS_BIND | SYS_LISTEN | SYS_ACCEPT | SYS_CONNECT => ExecutionPath::Hybrid,
            SYS_SENDTO | SYS_RECVFROM | SYS_SENDMSG | SYS_RECVMSG => ExecutionPath::Hybrid,
            SYS_FCNTL | SYS_IOCTL => ExecutionPath::Hybrid,
            SYS_PREAD64 | SYS_PWRITE64 | SYS_READV | SYS_WRITEV => ExecutionPath::Hybrid,

            // Legacy path - process control (8000-50000 cycles)
            SYS_FORK | SYS_VFORK | SYS_CLONE => ExecutionPath::Legacy,
            SYS_EXECVE => ExecutionPath::Legacy,
            SYS_WAIT4 => ExecutionPath::Legacy,
            SYS_KILL => ExecutionPath::Legacy,
            SYS_RT_SIGACTION | SYS_RT_SIGPROCMASK | SYS_RT_SIGRETURN => ExecutionPath::Legacy,

            // Default to hybrid for unknown syscalls
            _ => ExecutionPath::Hybrid,
        }
    }

    /// Translate and execute syscall
    pub fn translate(
        &mut self,
        syscall_num: i64,
        args: &[u64; 6],
    ) -> Result<i64, PosixXError> {
        let path = self.classify_syscall(syscall_num);

        match path {
            ExecutionPath::Fast => {
                self.fast_count += 1;
                self.execute_fast(syscall_num, args)
            }
            ExecutionPath::Hybrid => {
                self.hybrid_count += 1;
                self.execute_hybrid(syscall_num, args)
            }
            ExecutionPath::Legacy => {
                self.legacy_count += 1;
                self.execute_legacy(syscall_num, args)
            }
        }
    }

    /// Fast path execution (< 50 cycles)
    fn execute_fast(&self, syscall_num: i64, _args: &[u64; 6]) -> Result<i64, PosixXError> {
        match syscall_num {
            SYS_GETPID => {
                // Direct mapping to exo_std::process::getpid()
                // TODO: Implement actual call
                Ok(1)
            }
            SYS_GETUID | SYS_GETEUID => {
                // Direct mapping
                Ok(1000) // Placeholder UID
            }
            SYS_GETGID | SYS_GETEGID => {
                // Direct mapping
                Ok(1000) // Placeholder GID
            }
            SYS_GETPPID => {
                // Direct mapping
                Ok(1) // Placeholder PPID
            }
            SYS_CLOCK_GETTIME => {
                // Map to vDSO-like fast time
                // TODO: Implement via exo_std::time
                Ok(0)
            }
            _ => Err(PosixXError::NotSupported(syscall_num as i32)),
        }
    }

    /// Hybrid path execution (400-1000 cycles)
    fn execute_hybrid(&self, syscall_num: i64, args: &[u64; 6]) -> Result<i64, PosixXError> {
        match syscall_num {
            SYS_OPEN => {
                // Translate to capability-based open
                // 1. Get capability from cache (50 cycles) or resolve (2000 cycles)
                // 2. Open via capability
                let _path_ptr = args[0];
                let _flags = args[1] as i32;
                let _mode = args[2] as u32;

                // TODO: Implement via exo_std::fs
                Ok(3) // Placeholder fd
            }
            SYS_READ => {
                let _fd = args[0] as i32;
                let _buf = args[1];
                let _count = args[2] as usize;

                // TODO: Implement via exo_std::io::Read
                Ok(0)
            }
            SYS_WRITE => {
                let _fd = args[0] as i32;
                let _buf = args[1];
                let _count = args[2] as usize;

                // TODO: Implement via exo_std::io::Write
                Ok(0)
            }
            SYS_CLOSE => {
                let _fd = args[0] as i32;

                // TODO: Implement via exo_std::fs
                Ok(0)
            }
            SYS_PIPE => {
                // Map to Fusion Ring for IPC
                // TODO: Implement via exo_ipc
                Ok(0)
            }
            _ => Err(PosixXError::NotSupported(syscall_num as i32)),
        }
    }

    /// Legacy path execution (8000-50000 cycles)
    fn execute_legacy(&self, syscall_num: i64, args: &[u64; 6]) -> Result<i64, PosixXError> {
        match syscall_num {
            SYS_FORK => {
                // Emulate via clone() + shared memory setup
                // This is expensive but necessary for compatibility
                log::warn!("fork() called - using expensive emulation path");

                // TODO: Implement fork emulation
                Ok(0) // Return 0 in child, pid in parent
            }
            SYS_EXECVE => {
                let _pathname = args[0];
                let _argv = args[1];
                let _envp = args[2];

                // TODO: Implement via exo_std::process::exec
                Err(PosixXError::NotSupported(syscall_num as i32))
            }
            SYS_WAIT4 => {
                let _pid = args[0] as i32;
                let _status = args[1];
                let _options = args[2] as i32;
                let _rusage = args[3];

                // TODO: Implement via exo_std::process::wait
                Ok(0)
            }
            _ => Err(PosixXError::NotSupported(syscall_num as i32)),
        }
    }

    /// Get translation statistics
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.fast_count, self.hybrid_count, self.legacy_count)
    }
}

impl Default for SyscallTranslator {
    fn default() -> Self {
        Self::new()
    }
}
