//! Exec Syscalls - Program Execution
//!
//! Delegation to actual handlers in kernel/syscall/handlers/process.rs

use crate::syscall::handlers::process;

/// execve - Execute program
/// 
/// Delegate to actual implementation in process handlers
pub fn sys_execve(filename: usize, argv: usize, envp: usize) -> i64 {
    // Convert pointers to proper types
    let pathname = filename as *const i8;
    let argv_ptr = argv as *const *const i8;
    let envp_ptr = envp as *const *const i8;
    
    // Call actual implementation
    match process::sys_execve(pathname, argv_ptr, envp_ptr) {
        Ok(_) => {
            // execve only returns on error
            // On success, this thread is replaced by new program
            unreachable!("execve returned Ok - should not happen");
        }
        Err(e) => {
            // Return errno on error
            use crate::memory::MemoryError;
            match e {
                MemoryError::NotFound => -2,        // ENOENT
                MemoryError::InvalidAddress => -14, // EFAULT
                MemoryError::PermissionDenied => -13, // EACCES
                MemoryError::NotSupported => -38,   // ENOSYS (if loader not ready)
                MemoryError::OutOfMemory => -12,    // ENOMEM
                _ => -5,                             // EIO
            }
        }
    }
}

/// execveat - Execute program at dirfd
/// 
/// For now, delegate to execve (dirfd handling requires VFS extensions)
pub fn sys_execveat(_dirfd: i32, pathname: usize, argv: usize, envp: usize, _flags: i32) -> i64 {
    // Simple delegation to execve for now
    // Full implementation would resolve pathname relative to dirfd
    sys_execve(pathname, argv, envp)
}
