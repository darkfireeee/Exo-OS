//! Syscall subsystem
//!
//! Provides fast syscall interface using SYSCALL/SYSRET

pub mod dispatch;

pub use dispatch::{
    dispatch_syscall, register_syscall, syscall_numbers, unregister_syscall, SyscallError,
    SyscallHandler,
};

pub mod handlers;
pub mod utils;

/// Syscall result type
pub type SyscallResult = Result<u64, SyscallError>;

/// Initialize syscall subsystem
pub unsafe fn init() {
    dispatch::init();
    handlers::init();
}
