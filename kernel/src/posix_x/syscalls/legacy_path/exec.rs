//! Exec Syscalls - Program Execution

/// execve - Execute program
pub fn sys_execve(_filename: usize, _argv: usize, _envp: usize) -> i64 {
    // Complex: Replace current process
    // - Load ELF binary
    // - Set up new address space
    // - Initialize stack with args/env
    // - Jump to entry point

    // Only returns on error
    -38 // ENOSYS
}

/// execveat - Execute program at dirfd
pub fn sys_execveat(_dirfd: i32, _pathname: usize, _argv: usize, _envp: usize, _flags: i32) -> i64 {
    -38 // ENOSYS
}
