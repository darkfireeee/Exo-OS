//! Fast Path Syscalls
//!
//! Optimized syscalls with minimal overhead - direct kernel calls

pub mod info;
pub mod process;
pub mod time;

// Re-exports
pub use info::{sys_getgid, sys_getpid, sys_getppid, sys_gettid, sys_getuid};
pub use process::{sys_getpriority, sys_setpriority};
pub use time::{sys_clock_gettime, sys_gettime, sys_nanosleep};
