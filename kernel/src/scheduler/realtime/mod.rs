//! Real-time scheduling support

pub mod deadline;
pub mod priorities;
pub mod latency;

pub use deadline::{DeadlineScheduler, Deadline};
pub use priorities::{RealtimePriority, RT_PRIORITY_MAX, RT_PRIORITY_MIN};
pub use latency::LatencyTracker;

use crate::scheduler::thread::Thread;
use alloc::vec::Vec;
use spin::Mutex;

/// Real-time scheduler
pub struct RealtimeScheduler {
    /// Real-time threads sorted by deadline
    rt_threads: Mutex<Vec<Thread>>,
}

impl RealtimeScheduler {
    pub fn new() -> Self {
        Self {
            rt_threads: Mutex::new(Vec::new()),
        }
    }
    
    /// Add real-time thread
    pub fn add_thread(&self, thread: Thread) {
        let mut threads = self.rt_threads.lock();
        threads.push(thread);
    }
    
    /// Get next real-time thread
    pub fn next_thread(&self) -> Option<Thread> {
        let mut threads = self.rt_threads.lock();
        if threads.is_empty() {
            None
        } else {
            Some(threads.remove(0))
        }
    }
}

impl Default for RealtimeScheduler {
    fn default() -> Self {
        Self::new()
    }
}
