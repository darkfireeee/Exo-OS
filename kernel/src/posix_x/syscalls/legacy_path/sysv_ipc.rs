//! System V IPC Syscalls

/// shmget - Get shared memory segment
pub fn sys_shmget(_key: i32, _size: usize, _shmflg: i32) -> i64 {
    -38 // ENOSYS
}

/// shmat - Attach shared memory
pub fn sys_shmat(_shmid: i32, _shmaddr: usize, _shmflg: i32) -> i64 {
    -38 // ENOSYS
}

/// shmdt - Detach shared memory
pub fn sys_shmdt(_shmaddr: usize) -> i64 {
    -38 // ENOSYS
}

/// shmctl - Control shared memory
pub fn sys_shmctl(_shmid: i32, _cmd: i32, _buf: usize) -> i64 {
    -38 // ENOSYS
}
