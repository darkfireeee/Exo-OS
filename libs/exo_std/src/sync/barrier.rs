// libs/exo_std/src/sync/barrier.rs
//! Barrier synchronization for thread coordination
//!
//! Allows N threads to wait for each other at a rendezvous point.

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::Result;
use crate::error::SyncError;

/// Synchronization barrier
///
/// # Example
/// ```no_run
/// use exo_std::sync::Barrier;
/// use exo_std::thread;
///
/// let barrier = Barrier::new(3);
///
/// for _ in 0..3 {
///     let b = &barrier;
///     thread::spawn(move || {
///         // Parallel work
///
///         // Wait at barrier
///         let result = b.wait();
///
///         if result.is_leader() {
///             // Only one thread executes this
///         }
///     });
/// }
/// ```
pub struct Barrier {
    /// Number of threads that must wait()
    num_threads: usize,
    /// Counter of arrived threads
    count: AtomicUsize,
    /// Current generation (to detect cycles)
    generation: AtomicUsize,
}

/// Result of a wait() on barrier
pub struct BarrierWaitResult {
    is_leader: bool,
}

impl Barrier {
    /// Create a new barrier for `n` threads
    ///
    /// # Panics
    /// Panics if `n` == 0
    pub fn new(n: usize) -> Self {
        assert!(n > 0, "Barrier count must be > 0");

        Self {
            num_threads: n,
            count: AtomicUsize::new(0),
            generation: AtomicUsize::new(0),
        }
    }

    /// Wait until all threads reach the barrier
    ///
    /// Returns a result indicating if this thread is the "leader"
    /// (the last to arrive). Exactly one thread will receive `is_leader() == true`.
    pub fn wait(&self) -> Result<BarrierWaitResult> {
        let gen = self.generation.load(Ordering::Acquire);

        // Increment counter
        let count = self.count.fetch_add(1, Ordering::AcqRel) + 1;

        if count < self.num_threads {
            // Not all arrived yet, wait
            self.wait_for_generation(gen)?;
            Ok(BarrierWaitResult { is_leader: false })
        } else {
            // Last thread: reset for next cycle
            self.count.store(0, Ordering::Release);
            self.generation.fetch_add(1, Ordering::Release);
            Ok(BarrierWaitResult { is_leader: true })
        }
    }

    /// Wait until generation changes
    fn wait_for_generation(&self, gen: usize) -> Result<()> {
        let mut backoff = super::mutex::Backoff::new();

        loop {
            let current_gen = self.generation.load(Ordering::Acquire);
            if current_gen != gen {
                return Ok(());
            }

            backoff.spin();
            if backoff.should_yield() {
                crate::syscall::thread::yield_now();
            }
            backoff.next();
        }
    }
}

impl BarrierWaitResult {
    /// Returns true if this thread is the leader (last arrived)
    #[inline]
    pub fn is_leader(&self) -> bool {
        self.is_leader
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_barrier_single_thread() {
        let barrier = Barrier::new(1);
        let result = barrier.wait().unwrap();
        assert!(result.is_leader());
    }

    #[test]
    fn test_barrier_creation() {
        let barrier = Barrier::new(5);
        assert_eq!(barrier.num_threads, 5);
    }

    #[test]
    #[should_panic(expected = "Barrier count must be > 0")]
    fn test_barrier_zero_threads() {
        let _barrier = Barrier::new(0);
    }
}
