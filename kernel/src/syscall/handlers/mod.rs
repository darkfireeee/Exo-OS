//! System Call Handlers - Phase 1 Minimal
//!
//! Phase 1: Only process + memory management syscalls
//! Phase 1b: Will add VFS syscalls (fs_*, io, etc.)
//! Phase 2: Will add IPC/network syscalls

// ⏸️ Phase 1b: pub mod fs_dir;
// ⏸️ Phase 1b: pub mod fs_events;
// ⏸️ Phase 1b: pub mod fs_fcntl;
// ⏸️ Phase 1b: pub mod fs_fifo;
// ⏸️ Phase 1b: pub mod fs_futex;
// ⏸️ Phase 1b: pub mod fs_link;
// ⏸️ Phase 1b: pub mod fs_ops;
// ⏸️ Phase 1b: pub mod fs_poll;
// ⏸️ Phase 1b: pub mod inotify;
// ⏸️ Phase 1b: pub mod io;
// ⏸️ Phase 2: pub mod ipc;
// ⏸️ Phase 2: pub mod ipc_sysv;
pub mod memory;
// ⏸️ Phase 2: pub mod net_socket;
pub mod process;
pub mod process_limits;
pub mod sched;
pub mod security;
pub mod signals;
pub mod sys_info;
pub mod time;

// Re-export commonly used types
// ⏸️ Phase 1b: pub use io::{Fd, FileFlags, FileStat};
// ⏸️ Phase 2: pub use ipc::IpcHandle;

/// Initialize all syscall handlers
pub fn init() {
    use crate::syscall::dispatch::{register_syscall, syscall_numbers::*};
    
    log::info!("[Phase 1b] Registering syscall handlers...");
    
    // Process management syscalls
    let _ = register_syscall(SYS_FORK, |_args| {
        log::info!("[SYSCALL] fork() called");
        match process::sys_fork() {
            Ok(child_pid) => {
                log::info!("[SYSCALL] fork() succeeded, child PID = {}", child_pid);
                Ok(child_pid)
            }
            Err(e) => {
                log::error!("[SYSCALL] fork() failed: {:?}", e);
                Err(memory_err_to_syscall_err(e))
            }
        }
    });
    
    let _ = register_syscall(SYS_EXIT, |args| {
        let exit_code = args[0] as i32;
        log::info!("[SYSCALL] exit({}) called", exit_code);
        process::sys_exit(exit_code);
    });
    
    let _ = register_syscall(SYS_WAIT4, |args| {
        let pid = args[0] as i64;
        let wstatus_ptr = args[1] as *mut i32;
        let options = args[2] as i32;
        
        log::info!("[SYSCALL] wait4({}, {:?}, {}) called", pid, wstatus_ptr, options);
        
        // Convert pid: -1 means any child
        let target_pid = if pid == -1 { u64::MAX } else { pid as u64 };
        
        let wait_options = process::WaitOptions {
            nohang: (options & 1) != 0, // WNOHANG = 1
            untraced: (options & 2) != 0, // WUNTRACED = 2
            continued: (options & 8) != 0, // WCONTINUED = 8
        };
        
        match process::sys_wait(target_pid, wait_options) {
            Ok((waited_pid, status)) => {
                if !wstatus_ptr.is_null() {
                    let wstatus = match status {
                        process::ProcessStatus::Exited(code) => (code & 0xFF) << 8,
                        process::ProcessStatus::Signaled(sig) => sig as i32,
                        _ => 0,
                    };
                    unsafe { *wstatus_ptr = wstatus; }
                }
                Ok(waited_pid)
            }
            Err(e) => Err(memory_err_to_syscall_err(e)),
        }
    });
    
    log::info!("  ✅ Process management: fork, exec, wait");
    log::info!("  ✅ Memory management: brk, mmap, munmap");
    log::info!("  ⏸️  VFS syscalls: Phase 1b");
    log::info!("  ⏸️  IPC/Network: Phase 2+");
}

/// Convert MemoryError to SyscallError
fn memory_err_to_syscall_err(
    e: crate::memory::MemoryError,
) -> crate::syscall::dispatch::SyscallError {
    use crate::memory::MemoryError;
    use crate::syscall::dispatch::SyscallError;

    match e {
        MemoryError::OutOfMemory => SyscallError::OutOfMemory,
        MemoryError::InvalidAddress => SyscallError::InvalidArgument,
        MemoryError::NotMapped => SyscallError::InvalidArgument,
        MemoryError::AlreadyMapped => SyscallError::InvalidArgument,
        MemoryError::PermissionDenied => SyscallError::PermissionDenied,
        MemoryError::AlignmentError => SyscallError::InvalidArgument,
        MemoryError::InvalidSize => SyscallError::InvalidArgument,
        MemoryError::InvalidParameter => SyscallError::InvalidArgument,
        MemoryError::Mfile => SyscallError::IoError,
        MemoryError::InternalError(_) => SyscallError::IoError,
        MemoryError::WouldBlock => SyscallError::WouldBlock,
        MemoryError::Timeout => SyscallError::Timeout,
        MemoryError::Interrupted => SyscallError::Interrupted,
        MemoryError::QueueFull => SyscallError::WouldBlock,
        MemoryError::NotSupported => SyscallError::NotSupported,
        MemoryError::NotFound => SyscallError::NotFound,
    }
}
