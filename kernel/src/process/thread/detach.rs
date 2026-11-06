// kernel/src/process/thread/detach.rs
//
// pthread_detach — marks a thread as detached.
// A detached thread automatically frees its resources upon termination.
// No join() is possible after detach().

use crate::process::core::tcb::ProcessThread;
use core::sync::atomic::Ordering;

/// Detach error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachError {
    /// Already detached.
    AlreadyDetached,
    /// Already joined (join in progress).
    JoinInProgress,
    /// Invalid pointer.
    InvalidThread,
}

/// Detaches target thread.
/// After a successful call, the thread frees its resources automatically.
///
/// # Safety
/// `target` must point to a valid ProcessThread.
pub unsafe fn thread_detach(target: *mut ProcessThread) -> Result<(), DetachError> {
    // SAFETY: target is guaranteed valid by caller.
    let thread = unsafe { &*target };

    // CAS: false -> true (atomic).
    match thread
        .detached
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
    {
        Ok(_) => Ok(()),
        Err(_) => Err(DetachError::AlreadyDetached),
    }
}

/// Checks if a thread is detached.
#[inline(always)]
pub fn is_detached(thread: &ProcessThread) -> bool {
    thread.detached.load(Ordering::Relaxed)
}
