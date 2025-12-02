//! Kernel Interface Module
//!
//! Bridges between POSIX syscalls and Exo-OS kernel subsystems

pub mod capability_cache;
pub mod ipc_bridge;
pub mod memory_bridge;
pub mod signal_daemon;
pub mod syscall_adapter;

pub use memory_bridge::{posix_brk, posix_mmap, posix_mprotect, posix_munmap};
pub use signal_daemon::{send_signal, SIGNAL_DAEMON};
pub use syscall_adapter::{execute_syscall, SyscallContext};

/// Initialize all kernel interface bridges
pub fn init() {
    memory_bridge::init();
    signal_daemon::init();
    syscall_adapter::init();
    log::info!("Kernel interface bridges initialized");
}
