//! System Call Handlers - Phase 1 Complete
//!
//! Phase 1: VFS, Process, Memory, Filesystem operations
//! Phase 2: IPC/network syscalls

pub mod fs_dir;
pub mod fs_events;
pub mod fs_fcntl;
pub mod fs_fifo;
pub mod fs_futex;
pub mod fs_link;
pub mod fs_ops;
pub mod fs_poll;
pub mod inotify;
pub mod io;
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
pub use io::{Fd, FileFlags, FileStat};
// ⏸️ Phase 2: pub use ipc::IpcHandle;

/// Initialize all syscall handlers
pub fn init() {
    use crate::syscall::dispatch::{register_syscall, syscall_numbers::*, SyscallError};
    
    log::info!("[Phase 1] Registering syscall handlers...");
    
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
    
    // VFS I/O syscalls
    log::info!("  [VFS] Registering I/O syscalls...");
    
    let _ = register_syscall(SYS_OPEN, |args| {
        use crate::syscall::utils::read_user_string;
        
        let path_ptr = args[0] as *const i8;
        let flags_raw = args[1] as u32;
        let mode = args[2] as u32;
        
        let path = unsafe {
            match read_user_string(path_ptr) {
                Ok(p) => p,
                Err(_) => return Err(SyscallError::InvalidArgument),
            }
        };
        
        // Convert flags
        let flags = io::FileFlags {
            read: (flags_raw & 3) == 0 || (flags_raw & 2) != 0, // O_RDONLY or O_RDWR
            write: (flags_raw & 3) != 0, // O_WRONLY or O_RDWR
            append: (flags_raw & 0o2000) != 0,
            create: (flags_raw & 0o100) != 0,
            truncate: (flags_raw & 0o1000) != 0,
            nonblock: (flags_raw & 0o4000) != 0,
        };
        
        match io::sys_open(&path, flags, mode) {
            Ok(fd) => Ok(fd as u64),
            Err(e) => Err(memory_err_to_syscall_err(e)),
        }
    });
    
    let _ = register_syscall(SYS_CLOSE, |args| {
        let fd = args[0] as i32;
        match io::sys_close(fd) {
            Ok(_) => Ok(0),
            Err(e) => Err(memory_err_to_syscall_err(e)),
        }
    });
    
    let _ = register_syscall(SYS_READ, |args| {
        let fd = args[0] as i32;
        let buf_ptr = args[1] as *mut u8;
        let count = args[2] as usize;
        
        let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
        
        match io::sys_read(fd, buf) {
            Ok(n) => Ok(n as u64),
            Err(e) => Err(memory_err_to_syscall_err(e)),
        }
    });
    
    let _ = register_syscall(SYS_WRITE, |args| {
        let fd = args[0] as i32;
        let buf_ptr = args[1] as *const u8;
        let count = args[2] as usize;
        
        let buf = unsafe { core::slice::from_raw_parts(buf_ptr, count) };
        
        match io::sys_write(fd, buf) {
            Ok(n) => Ok(n as u64),
            Err(e) => Err(memory_err_to_syscall_err(e)),
        }
    });
    
    let _ = register_syscall(SYS_LSEEK, |args| {
        use io::SeekWhence;
        
        let fd = args[0] as i32;
        let offset = args[1] as i64;
        let whence = args[2] as i32;
        
        let seek_whence = match whence {
            0 => SeekWhence::Start,
            1 => SeekWhence::Current,
            2 => SeekWhence::End,
            _ => return Err(SyscallError::InvalidArgument),
        };
        
        match io::sys_seek(fd, offset, seek_whence) {
            Ok(new_offset) => Ok(new_offset as u64),
            Err(e) => Err(memory_err_to_syscall_err(e)),
        }
    });
    
    let _ = register_syscall(SYS_STAT, |args| {
        use crate::syscall::utils::read_user_string;
        
        let path_ptr = args[0] as *const i8;
        let stat_ptr = args[1] as *mut io::FileStat;
        
        let path = unsafe {
            match read_user_string(path_ptr) {
                Ok(p) => p,
                Err(_) => return Err(SyscallError::InvalidArgument),
            }
        };
        
        match io::sys_stat(&path) {
            Ok(stat) => {
                unsafe { *stat_ptr = stat; }
                Ok(0)
            }
            Err(e) => Err(memory_err_to_syscall_err(e)),
        }
    });
    
    let _ = register_syscall(SYS_FSTAT, |args| {
        let fd = args[0] as i32;
        let stat_ptr = args[1] as *mut io::FileStat;
        
        match io::sys_fstat(fd) {
            Ok(stat) => {
                unsafe { *stat_ptr = stat; }
                Ok(0)
            }
            Err(e) => Err(memory_err_to_syscall_err(e)),
        }
    });
    
    log::info!("  ✅ VFS I/O: open, read, write, close, lseek, stat, fstat");
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
