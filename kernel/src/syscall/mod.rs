//! Syscall subsystem
//! 
//! Provides fast syscall interface using SYSCALL/SYSRET

pub mod dispatch;

pub use dispatch::{
    SyscallHandler, SyscallError,
    register_syscall, unregister_syscall,
    dispatch_syscall, syscall_numbers,
    init,
};

/// Syscall result type
pub type SyscallResult = Result<u64, SyscallError>;
