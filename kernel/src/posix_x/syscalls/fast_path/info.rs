//! Process/Thread Info Syscalls
//!
//! Fast path for process/thread identification with real process state integration

use crate::posix_x::core::process_state::{current_process_state, ProcessState};
use core::sync::atomic::{AtomicU64, Ordering};

/// Global PID counter (simple for now)
static NEXT_PID: AtomicU64 = AtomicU64::new(2); // Start at 2 (1 is init)
static CURRENT_PID: AtomicU64 = AtomicU64::new(1);
static CURRENT_PPID: AtomicU64 = AtomicU64::new(0);
static CURRENT_UID: AtomicU64 = AtomicU64::new(1000);
static CURRENT_GID: AtomicU64 = AtomicU64::new(1000);

/// Get process ID
pub fn sys_getpid() -> i64 {
    // Try to get from process state, fallback to atomic
    if let Some(state) = current_process_state() {
        let state = state.read();
        state.pid as i64
    } else {
        CURRENT_PID.load(Ordering::Relaxed) as i64
    }
}

/// Get parent process ID
pub fn sys_getppid() -> i64 {
    if let Some(state) = current_process_state() {
        let state = state.read();
        state.ppid as i64
    } else {
        CURRENT_PPID.load(Ordering::Relaxed) as i64
    }
}

/// Get thread ID
pub fn sys_gettid() -> i64 {
    // For now, same as PID (single-threaded)
    // TODO: Real thread ID when threading implemented
    sys_getpid()
}

/// Get user ID
pub fn sys_getuid() -> i64 {
    if let Some(state) = current_process_state() {
        let state = state.read();
        state.uid as i64
    } else {
        CURRENT_UID.load(Ordering::Relaxed) as i64
    }
}

/// Get group ID
pub fn sys_getgid() -> i64 {
    if let Some(state) = current_process_state() {
        let state = state.read();
        state.gid as i64
    } else {
        CURRENT_GID.load(Ordering::Relaxed) as i64
    }
}

/// Get effective user ID
pub fn sys_geteuid() -> i64 {
    if let Some(state) = current_process_state() {
        let state = state.read();
        state.euid as i64
    } else {
        CURRENT_UID.load(Ordering::Relaxed) as i64
    }
}

/// Get effective group ID
pub fn sys_getegid() -> i64 {
    if let Some(state) = current_process_state() {
        let state = state.read();
        state.egid as i64
    } else {
        CURRENT_GID.load(Ordering::Relaxed) as i64
    }
}

/// Set process credentials (for initialization)
pub fn set_process_credentials(pid: u64, ppid: u64, uid: u64, gid: u64) {
    CURRENT_PID.store(pid, Ordering::Relaxed);
    CURRENT_PPID.store(ppid, Ordering::Relaxed);
    CURRENT_UID.store(uid, Ordering::Relaxed);
    CURRENT_GID.store(gid, Ordering::Relaxed);
}

/// Allocate new PID
pub fn allocate_pid() -> u64 {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}
