//! POSIX-X Syscalls Module
//!
//! Three-tier syscall implementation architecture:
//! - **fast_path**: Optimized syscalls (getpid, gettid, etc.)
//! - **hybrid_path**: Mixed native/emulated (I/O, sockets)
//! - **legacy_path**: Full POSIX emulation (fork, exec, SysV IPC)

pub mod fast_path;
pub mod hybrid_path;
pub mod legacy_path;

/// Syscall tier classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallTier {
    /// Fast path: Direct kernel calls, minimal overhead
    Fast,
    /// Hybrid: Some translation, moderate overhead
    Hybrid,
    /// Legacy: Full emulation, higher overhead
    Legacy,
}

/// Get the tier for a syscall number
pub fn get_syscall_tier(syscall_num: usize) -> SyscallTier {
    match syscall_num {
        // Fast path syscalls (0-50)
        39 | 110 | 186 => SyscallTier::Fast, // getpid, getppid, gettid
        96 | 99 | 201 | 228 | 229 => SyscallTier::Fast, // time syscalls

        // Hybrid path syscalls (I/O, networking)
        0..=20 | 40..=45 | 50..=60 => SyscallTier::Hybrid, // I/O, sockets, stat

        // Legacy path (complex emulation)
        57 | 58 | 59 => SyscallTier::Legacy, // fork, vfork, exec
        _ => SyscallTier::Hybrid,            // Default to hybrid
    }
}

/// Syscall tier statistics
pub struct TierStats {
    pub fast_count: u64,
    pub hybrid_count: u64,
    pub legacy_count: u64,
}

impl TierStats {
    pub const fn new() -> Self {
        Self {
            fast_count: 0,
            hybrid_count: 0,
            legacy_count: 0,
        }
    }
}

// Re-exports for convenience
pub use fast_path::{sys_getpid, sys_gettid, sys_gettime};
pub use hybrid_path::{sys_close, sys_open, sys_read, sys_write};
pub use legacy_path::{sys_execve, sys_fork};
