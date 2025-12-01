//! Event and Signal FD Handlers
//!
//! Implements eventfd, eventfd2, signalfd, signalfd4.

pub fn sys_eventfd(initval: u32) -> i32 {
    sys_eventfd2(initval, 0)
}

pub fn sys_eventfd2(initval: u32, flags: i32) -> i32 {
    log::info!("sys_eventfd2: initval={}, flags={:#x}", initval, flags);
    // Stub: return a fake FD
    // In reality, this would create a new file descriptor pointing to an event counter.
    100
}

pub fn sys_signalfd(fd: i32, mask: *const u64, sigsetsize: usize) -> i32 {
    sys_signalfd4(fd, mask, sigsetsize, 0)
}

pub fn sys_signalfd4(fd: i32, mask: *const u64, sigsetsize: usize, flags: i32) -> i32 {
    log::info!(
        "sys_signalfd4: fd={}, size={}, flags={:#x}",
        fd,
        sigsetsize,
        flags
    );
    // Stub: return a fake FD
    // In reality, this would create a new file descriptor that receives signals.
    if !mask.is_null() {
        // Read mask...
    }
    101
}
