// libs/exo_std/src/sync/condvar.rs
//! Condition variable for complex synchronization
//!
//! Allows threads to efficiently wait for a condition to be satisfied.

use core::sync::atomic::{AtomicU32, Ordering};
use core::time::Duration;
use super::mutex::{Mutex, MutexGuard};
use crate::Result;
use crate::error::SyncError;

/// Condition variable for thread coordination
///
/// # Example
/// ```no_run
/// use exo_std::sync::{Mutex, Condvar};
///
/// let pair = (Mutex::new(false), Condvar::new());
/// let &(ref lock, ref cvar) = &pair;
///
/// // Thread 1
/// {
///     let mut started = lock.lock().unwrap();
///     *started = true;
///     cvar.notify_one();
/// }
///
/// // Thread 2
/// {
///     let mut started = lock.lock().unwrap();
///     while !*started {
///         started = cvar.wait(started).unwrap();
///     }
/// }
/// ```
pub struct Condvar {
    /// Counter for wake-ups
    seq: AtomicU32,
}

impl Condvar {
    /// Create a new Condvar
    #[inline]
    pub const fn new() -> Self {
        Self {
            seq: AtomicU32::new(0),
        }
    }

    /// Wait until the condition is signaled
    ///
    /// Temporarily releases the mutex and waits. The mutex is reacquired
    /// before returning.
    ///
    /// # Panics
    /// Panics if the mutex is poisoned on wakeup
    #[inline]
    pub fn wait<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
    ) -> Result<MutexGuard<'a, T>> {
        let mutex = guard.mutex;
        let seq = self.seq.load(Ordering::Acquire);

        // Release the mutex
        drop(guard);

        // Optimized busy wait
        let mut backoff = super::mutex::Backoff::new();
        while self.seq.load(Ordering::Acquire) == seq {
            backoff.spin();
            if backoff.should_yield() {
                crate::syscall::thread::yield_now();
            }
            backoff.next();
        }

        // Reacquire the mutex
        mutex.lock().map_err(|_| SyncError::Poisoned.into())
    }

    /// Wait with timeout
    ///
    /// Returns true if woken by notify, false if timeout
    #[inline]
    pub fn wait_timeout<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        dur: Duration,
    ) -> Result<(MutexGuard<'a, T>, bool)> {
        let mutex = guard.mutex;
        let seq = self.seq.load(Ordering::Acquire);
        let start = crate::time::Instant::now();

        drop(guard);

        let mut backoff = super::mutex::Backoff::new();
        loop {
            if self.seq.load(Ordering::Acquire) != seq {
                // Signal received
                let guard = mutex.lock().map_err(|_| SyncError::Poisoned)?;
                return Ok((guard, true));
            }

            if start.elapsed() >= dur {
                // Timeout
                let guard = mutex.lock().map_err(|_| SyncError::Poisoned)?;
                return Ok((guard, false));
            }

            backoff.spin();
            if backoff.should_yield() {
                crate::syscall::thread::yield_now();
            }
            backoff.next();
        }
    }

    /// Wait while a condition is true
    ///
    /// Equivalent to:
    /// ```ignore
    /// while !condition() {
    ///     guard = condvar.wait(guard)?;
    /// }
    /// ```
    #[inline]
    pub fn wait_while<'a, T, F>(
        &self,
        mut guard: MutexGuard<'a, T>,
        mut condition: F,
    ) -> Result<MutexGuard<'a, T>>
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut *guard) {
            guard = self.wait(guard)?;
        }
        Ok(guard)
    }

    /// Wake one waiting thread
    #[inline]
    pub fn notify_one(&self) {
        self.seq.fetch_add(1, Ordering::Release);
    }

    /// Wake all waiting threads
    #[inline]
    pub fn notify_all(&self) {
        self.seq.fetch_add(1, Ordering::Release);
    }
}

impl Default for Condvar {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_condvar_basic() {
        let cv = Condvar::new();
        cv.notify_one();
        cv.notify_all();
    }

    #[test]
    fn test_condvar_with_mutex() {
        let mutex = Mutex::new(false);
        let condvar = Condvar::new();

        let guard = mutex.lock().unwrap();
        condvar.notify_one();
        let _result = condvar.wait_timeout(guard, Duration::from_millis(10)).unwrap();
    }

    #[test]
    fn test_condvar_notify() {
        let cv = Condvar::new();

        // Test notify functions
        cv.notify_one();
        cv.notify_all();

        // Verify seq counter increases
        let seq1 = cv.seq.load(Ordering::Acquire);
        cv.notify_one();
        let seq2 = cv.seq.load(Ordering::Acquire);
        assert!(seq2 > seq1);
    }
}
