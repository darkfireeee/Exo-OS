//! Wait Queue
//!
//! A mechanism for threads to sleep until an event occurs.

use crate::scheduler::{ThreadId, SCHEDULER};
use crate::time::Duration;
use alloc::vec::Vec;
use spin::Mutex;

/// A queue of waiting threads
pub struct WaitQueue {
    waiting: Mutex<Vec<ThreadId>>,
}

impl WaitQueue {
    /// Create a new wait queue
    pub const fn new() -> Self {
        Self {
            waiting: Mutex::new(Vec::new()),
        }
    }

    /// Block the current thread and add it to the wait queue
    pub fn wait(&self) {
        let tid = SCHEDULER.current_thread_id();
        if let Some(id) = tid {
            {
                let mut waiting = self.waiting.lock();
                waiting.push(id);
            }
            // Block current thread (it will be moved to blocked_threads list by scheduler)
            SCHEDULER.block_current();
        }
    }

    /// Block the current thread with timeout
    /// Returns true if notified, false if timed out
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let tid = SCHEDULER.current_thread_id();
        if let Some(id) = tid {
            // Add to waiting list
            {
                let mut waiting = self.waiting.lock();
                waiting.push(id);
            }

            // Create timer to wake us up
            let timer_id = crate::time::set_timer(timeout.as_ns() as u64, move || {
                SCHEDULER.unblock_thread(id);
            });

            // Block current thread
            SCHEDULER.block_current();

            // We are back (woken by notify or timer)

            // Cancel timer (if we woke up due to notify)
            // Note: cancel_timer is safe to call even if timer already fired
            crate::time::cancel_timer(timer_id);

            // Check if we are still in the waiting list
            let mut waiting = self.waiting.lock();
            if let Some(pos) = waiting.iter().position(|&x| x == id) {
                // We are still in the list -> Timed out
                waiting.remove(pos);
                return false;
            } else {
                // We were removed from the list -> Notified
                return true;
            }
        }
        false
    }

    /// Wake up one waiting thread
    pub fn notify_one(&self) {
        let mut waiting = self.waiting.lock();
        if let Some(tid) = waiting.pop() {
            SCHEDULER.unblock_thread(tid);
        }
    }

    /// Wake up all waiting threads
    pub fn notify_all(&self) {
        let mut waiting = self.waiting.lock();
        for tid in waiting.drain(..) {
            SCHEDULER.unblock_thread(tid);
        }
    }
}
