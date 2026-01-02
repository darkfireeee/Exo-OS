//! Fork Syscalls - Process Creation
//!
//! Delegation to actual handlers in kernel/syscall/handlers/process.rs

use crate::syscall::handlers::process;

/// fork - Create child process
/// 
/// Delegate to actual implementation in process handlers
pub fn sys_fork() -> i64 {
    match process::sys_fork() {
        Ok(child_pid) => child_pid as i64,
        Err(e) => {
            // Return errno on error
            use crate::memory::MemoryError;
            match e {
                MemoryError::OutOfMemory => -12,    // ENOMEM
                MemoryError::NotSupported => -38,   // ENOSYS
                MemoryError::InvalidAddress => -14, // EFAULT
                _ => -5,                             // EIO
            }
        }
    }
}

/// vfork - Create child (shared memory)
/// 
/// In modern systems, vfork is typically the same as fork with COW
/// We delegate to fork implementation
pub fn sys_vfork() -> i64 {
    // vfork has stricter semantics (child runs first, parent blocked)
    // For now, we just delegate to fork (COW provides similar performance)
    sys_fork()
}

/// clone - Create thread/process
/// 
/// Handles both thread creation (CLONE_THREAD) and process creation
pub fn sys_clone(flags: u64, stack: usize, _ptid: usize, _ctid: usize, _newtls: usize) -> i64 {
    // Check if this is thread creation or process creation
    const CLONE_THREAD: u64 = 0x00010000;
    const CLONE_VM: u64 = 0x00000100;
    
    // Convert to correct signature for process::sys_clone(flags: u32, stack: Option<usize>)
    let stack_opt = if stack == 0 { None } else { Some(stack) };
    
    if (flags & CLONE_THREAD) != 0 {
        // Thread creation
        match process::sys_clone(flags as u32, stack_opt) {
            Ok(tid) => tid as i64,
            Err(e) => {
                use crate::memory::MemoryError;
                match e {
                    MemoryError::OutOfMemory => -12,
                    MemoryError::NotSupported => -38,
                    MemoryError::InvalidAddress => -14,
                    _ => -5,
                }
            }
        }
    } else if (flags & CLONE_VM) == 0 {
        // Process creation (new address space)
        // This is similar to fork but with more control
        sys_fork()
    } else {
        // VM shared but not thread - unusual, treat as thread
        match process::sys_clone(flags as u32, stack_opt) {
            Ok(tid) => tid as i64,
            Err(e) => {
                use crate::memory::MemoryError;
                match e {
                    MemoryError::OutOfMemory => -12,
                    MemoryError::NotSupported => -38,
                    _ => -5,
                }
            }
        }
    }
}
