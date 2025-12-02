//! Process Priority Syscalls

/// Get process priority
pub fn sys_getpriority(_which: i32, _who: i32) -> i64 {
    // Would query scheduler
    0 // Default priority
}

/// Set process priority
pub fn sys_setpriority(_which: i32, _who: i32, _prio: i32) -> i64 {
    // Would update scheduler
    0 // Success
}
