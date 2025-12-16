//! Kernel Interface Module
//!
//! Bridges between POSIX syscalls and Exo-OS kernel subsystems

pub mod capability_cache;
// pub mod ipc_bridge;      // ⏸️ Phase 2: IPC bridge (require crate::ipc)
pub mod memory_bridge;
// pub mod signal_daemon;   // ⏸️ Phase 2: Signal daemon (require crate::ipc)
pub mod syscall_adapter;

pub use memory_bridge::{posix_brk, posix_mmap, posix_mprotect, posix_munmap};
// pub use signal_daemon::{send_signal, SIGNAL_DAEMON};  // ⏸️ Phase 2
pub use syscall_adapter::{execute_syscall, SyscallContext};

/// Initialize all kernel interface bridges
pub fn init() {
    memory_bridge::init();
    // signal_daemon::init();  // ⏸️ Phase 2
    syscall_adapter::init();
    log::info!("Kernel interface bridges initialized");
}
