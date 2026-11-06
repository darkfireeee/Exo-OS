// kernel/src/process/thread/join.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// pthread_join / thread_join — waiting for thread termination (futex-based)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Mechanism:
//   • ProcessThread::join_done: AtomicBool indicating completion.
//   • Joiner: poll join_done. If false -> wait via wait_queue.
//   • Terminated thread: sets join_result + join_done=true + wake_all.
//
// join() is CONSIDERED INTERRUPTIBLE by signals.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::process::core::tcb::ProcessThread;
use crate::scheduler::core::task::ThreadControlBlock;
use crate::scheduler::sync::wait_queue::WaitQueue;
use core::sync::atomic::Ordering;

/// Join table: one WaitQueue for all possible TIDs.
/// Used by do_exit_thread() to wake joiners.
static JOIN_WAIT: WaitQueue = WaitQueue::new();

/// Join error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinError {
    /// Target thread is detached.
    Detached,
    /// Already joined by another thread.
    AlreadyJoined,
    /// Interrupted by a signal.
    Interrupted,
    /// Invalid thread pointer.
    InvalidThread,
}

/// Waits for termination of target thread and retrieves its return value.
///
/// # Safety
/// `target` must point to a valid, non-freed ProcessThread.
/// `caller_tcb` must be the TCB of the calling thread.
pub unsafe fn thread_join(
    target: *const ProcessThread,
    caller_tcb: &ThreadControlBlock,
) -> Result<u64, JoinError> {
    // SAFETY: caller guarantees that target is valid.
    let target = unsafe { &*target };

    // Verify that the thread is not detached.
    if target.detached.load(Ordering::Relaxed) {
        return Err(JoinError::Detached);
    }

    // Wait for completion.
    // Spurious-wakeup-safe loop.
    loop {
        if target.join_done.load(Ordering::Acquire) {
            let result = target.join_result.load(Ordering::Acquire);
            return Ok(result);
        }
        // Check for pending signal (EINTR).
        if caller_tcb.has_signal_pending() {
            return Err(JoinError::Interrupted);
        }
        // Block on the wait queue.
        // SAFETY: JOIN_WAIT EmergencyPool (WAITQ-01); caller_tcb is current TCB, no active &mut alias.
        unsafe {
            JOIN_WAIT.wait_interruptible(caller_tcb as *const _ as *mut _);
        }
    }
}

/// Wakes all joiners of a thread (called by do_exit_thread).
pub fn wake_joiners() {
    JOIN_WAIT.notify_all();
}
