//! Signal Syscalls

use crate::posix_x::translation::signals_to_msgs::Signal;

/// kill - Send signal to process
pub fn sys_kill(pid: i32, sig: i32) -> i64 {
    if pid <= 0 {
        return -22; // EINVAL
    }

    // Convert to Signal enum
    let _signal = Signal::from_i32(sig);

    // Would send IPC message to process
    // For now, return success
    0
}

/// sigaction - Set signal handler
pub fn sys_sigaction(_signum: i32, _act: usize, _oldact: usize) -> i64 {
    // Would update signal handler table
    0
}

/// sigprocmask - Set signal mask
pub fn sys_sigprocmask(_how: i32, _set: usize, _oldset: usize) -> i64 {
    0
}
