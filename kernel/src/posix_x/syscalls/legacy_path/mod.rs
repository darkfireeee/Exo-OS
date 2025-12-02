//! Legacy Path Syscalls
//!
//! Full POSIX emulation for complex syscalls

pub mod exec;
pub mod fork;
pub mod sysv_ipc;

// Re-exports
pub use exec::{sys_execve, sys_execveat};
pub use fork::{sys_clone, sys_fork, sys_vfork};
pub use sysv_ipc::{sys_shmat, sys_shmctl, sys_shmdt, sys_shmget};
