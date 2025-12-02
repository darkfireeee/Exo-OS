//! Hybrid Path Syscalls
//!
//! Mix of native Exo-OS and POSIX emulation

pub mod io;
pub mod memory;
pub mod signals;
pub mod socket;
pub mod stat;

// Re-exports
pub use io::{sys_close, sys_lseek, sys_open, sys_read, sys_write};
pub use memory::{sys_brk, sys_mmap, sys_mprotect, sys_munmap};
pub use signals::{sys_kill, sys_sigaction};
pub use socket::{sys_accept, sys_bind, sys_connect, sys_listen, sys_socket};
pub use stat::{sys_fstat, sys_lstat, sys_stat};
