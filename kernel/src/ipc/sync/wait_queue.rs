// ipc/sync/wait_queue.rs — IPC wait queue (scheduler wrapper) for Exo-OS
//
// This module provides an IPC-specific wait queue wrapping the
// scheduler WaitQueue (scheduler::sync::wait_queue) with IPC-specific
// logic: timeout handling, ChannelId identification,
// dedicated statistics, and configurable wakeup policy.
//
// RULE WAITQ-01: WaitNode comes from the scheduler EmergencyPool.
// This module does not duplicate logic — it delegates to the scheduler.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::{ChannelId, IpcError};

#[inline]
fn timeout_expired(enqueued_at: u64, timeout_ns: u64) -> bool {
    if timeout_ns == 0 {
        return false;
    }
    crate::scheduler::timer::clock::monotonic_ns().saturating_sub(enqueued_at) >= timeout_ns
}

// ---------------------------------------------------------------------------
// Wakeup policy
// ---------------------------------------------------------------------------

/// Wakeup policy of an IpcWaitQueue
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WakePolicy {
    /// Wake up exactly one waiter (FIFO)
    One = 0,
    /// Wake up all waiters
    All = 1,
    /// Wake up up to N waiters
    UpToN = 2,
}

// ---------------------------------------------------------------------------
// IPC Waiter
// ---------------------------------------------------------------------------

/// Maximum number of waiters per IpcWaitQueue
pub const MAX_IPC_WAITERS: usize = 64;

/// Wakeup reason of a waiter
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WakeReason {
    /// Normal wakeup (condition satisfied)
    Signaled = 0,
    /// Timeout expired
    Timeout = 1,
    /// Channel/resource closed
    Closed = 2,
    /// Interruption (signal or cancellation)
    Interrupted = 3,
}

impl WakeReason {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Timeout,
            2 => Self::Closed,
            3 => Self::Interrupted,
            _ => Self::Signaled,
        }
    }
}

/// A waiter in the IPC queue
#[repr(C, align(64))]
pub struct IpcWaiter {
    /// Waiting thread (opaque TaskId)
    pub thread_id: AtomicU64,
    /// The waiter is active (enrolled in the queue)
    pub active: AtomicBool,
    /// Woken
    pub woken: AtomicBool,
    /// Wakeup reason
    pub reason: AtomicU32,
    /// Sequence number (anti-spurious)
    pub seq: AtomicU32,
    /// Enrollment timestamp (ns since boot)
    pub enqueued_at: AtomicU64,
    /// Timeout (0 = infinite, in nanoseconds since enqueued_at)
    pub timeout_ns: AtomicU64,
    _pad: [u8; 16],
}

// SAFETY: all fields are atomic
unsafe impl Sync for IpcWaiter {}
unsafe impl Send for IpcWaiter {}

impl IpcWaiter {
    pub const fn new() -> Self {
        Self {
            thread_id: AtomicU64::new(0),
            active: AtomicBool::new(false),
            woken: AtomicBool::new(false),
            reason: AtomicU32::new(0),
            seq: AtomicU32::new(0),
            enqueued_at: AtomicU64::new(0),
            timeout_ns: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn is_woken(&self) -> bool {
        self.woken.load(Ordering::Acquire)
    }

    pub fn try_wake(&self, reason: WakeReason) -> bool {
        if !self.is_active() {
            return false;
        }
        if self
            .woken
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.reason.store(reason as u32, Ordering::Release);
            true
        } else {
            false
        }
    }

    pub fn wake(&self, reason: WakeReason) {
        let _ = self.try_wake(reason);
    }

    pub fn dequeue(&self) {
        self.active.store(false, Ordering::Release);
        self.seq.fetch_add(1, Ordering::Relaxed);
    }

    pub fn wake_reason(&self) -> WakeReason {
        WakeReason::from_u32(self.reason.load(Ordering::Acquire))
    }
}

// ---------------------------------------------------------------------------
// IpcWaitQueue — IPC wait queue
// ---------------------------------------------------------------------------

/// IPC wait queue for a channel or resource.
///
/// Stores up to MAX_IPC_WAITERS waiting threads.
/// Wakeups respect the configured policy.
#[repr(C, align(64))]
pub struct IpcWaitQueue {
    /// Associated channel/resource identifier (for correlation)
    pub channel_id: ChannelId,
    /// Static array of waiters
    waiters: [IpcWaiter; MAX_IPC_WAITERS],
    /// Number of active waiters
    count: AtomicU32,
    /// Default wakeup policy
    policy: AtomicU32,
    /// Statistical counters
    pub total_waits: AtomicU64,
    pub total_woken: AtomicU64,
    pub total_timeouts: AtomicU64,
    _pad: [u8; 16],
}

// SAFETY: IpcWaiter is Sync
unsafe impl Sync for IpcWaitQueue {}
unsafe impl Send for IpcWaitQueue {}

impl IpcWaitQueue {
    pub const fn new(channel_id: ChannelId) -> Self {
        const INIT_WAITER: IpcWaiter = IpcWaiter::new();
        Self {
            channel_id,
            waiters: [INIT_WAITER; MAX_IPC_WAITERS],
            count: AtomicU32::new(0),
            policy: AtomicU32::new(WakePolicy::One as u32),
            total_waits: AtomicU64::new(0),
            total_woken: AtomicU64::new(0),
            total_timeouts: AtomicU64::new(0),
            _pad: [0u8; 16],
        }
    }

    pub fn set_policy(&self, policy: WakePolicy) {
        self.policy.store(policy as u32, Ordering::Relaxed);
    }

    pub fn waiter_count(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // Waiter enrollment
    // -----------------------------------------------------------------------

    /// Enrolls thread `thread_id` as waiter with an optional timeout.
    /// Returns the waiter index, or `None` if the queue is full.
    pub fn enqueue(&self, thread_id: u64, timeout_ns: u64, now_ns: u64) -> Option<usize> {
        for i in 0..MAX_IPC_WAITERS {
            // T8-015 FIX: atomic CAS false -> true to reserve slot exclusively
            if self.waiters[i]
                .active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.waiters[i]
                    .thread_id
                    .store(thread_id, Ordering::Relaxed);
                self.waiters[i]
                    .timeout_ns
                    .store(timeout_ns, Ordering::Relaxed);
                self.waiters[i].enqueued_at.store(now_ns, Ordering::Relaxed);
                self.waiters[i].woken.store(false, Ordering::Relaxed);
                self.waiters[i].reason.store(0, Ordering::Release);
                self.count.fetch_add(1, Ordering::Relaxed);
                self.total_waits.fetch_add(1, Ordering::Relaxed);
                return Some(i);
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Waiting
    // -----------------------------------------------------------------------

    /// Waits to be woken by actually blocking the thread via the scheduler.
    ///
    /// STRATEGY: short spin (<= 64 iterations) for immediate wakeups,
    /// then real blocking via `sched_hooks::block_current()`.
    ///
    /// The caller must have called `enqueue()` BEFORE and retrieved `waiter_idx`.
    /// Upon return (Signaled or error), the waiter is unregistered.
    ///
    /// # Return
    /// - `Ok(WakeReason)` — cause of wakeup
    /// - `Err(IpcError::Timeout)` — timeout expired
    pub fn wait_for_wake(
        &self,
        waiter_idx: usize,
        check_closed: &dyn Fn() -> bool,
    ) -> Result<WakeReason, IpcError> {
        if waiter_idx >= MAX_IPC_WAITERS {
            return Err(IpcError::InvalidHandle);
        }

        let w = &self.waiters[waiter_idx];
        let tid = w.thread_id.load(Ordering::Relaxed);
        let timeout_ns = w.timeout_ns.load(Ordering::Relaxed);
        let enqueued_at = w.enqueued_at.load(Ordering::Relaxed);
        let mut spin_count: u32 = 0;
        const SPIN_BEFORE_BLOCK: u32 = 64;

        loop {
            // 1. Check woken flag first (avoids blocking if fast wakeup).
            if w.is_woken() {
                let reason = w.wake_reason();
                w.dequeue();
                self.count.fetch_sub(1, Ordering::Relaxed);

                return match reason {
                    WakeReason::Signaled => {
                        self.total_woken.fetch_add(1, Ordering::Relaxed);
                        Ok(WakeReason::Signaled)
                    }
                    WakeReason::Closed => Err(IpcError::Closed),
                    WakeReason::Timeout => {
                        self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                        Err(IpcError::Timeout)
                    }
                    WakeReason::Interrupted => Err(IpcError::Closed),
                };
            }

            // 2. Check closure.
            if check_closed() {
                w.wake(WakeReason::Closed);
                w.dequeue();
                self.count.fetch_sub(1, Ordering::Relaxed);
                return Err(IpcError::Closed);
            }

            // 3. Check timeout with real monotonic clock.
            if timeout_expired(enqueued_at, timeout_ns) && !w.is_woken() {
                w.wake(WakeReason::Timeout);
                w.dequeue();
                self.count.fetch_sub(1, Ordering::Relaxed);
                self.total_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(IpcError::Timeout);
            }

            // 4. Short spin phase (<= SPIN_BEFORE_BLOCK iterations) for fast wakeups.
            if spin_count < SPIN_BEFORE_BLOCK {
                core::hint::spin_loop();
                spin_count += 1;
                continue;
            }

            // 5. A finite wait must not sleep without an armed timer: yield
            // the CPU then recheck deadline. Infinite waits can
            // use true scheduler sleep until explicit wake.
            if timeout_ns != 0 {
                unsafe {
                    let _ = crate::scheduler::core::switch::cooperative_reschedule();
                }
                spin_count = SPIN_BEFORE_BLOCK;
                continue;
            }

            // 6. Real blocking via the scheduler.
            // RULE PREEMPT-BLOCK (B6): blocking with active PreemptGuard = guaranteed deadlock.
            debug_assert!(
                crate::scheduler::core::preempt::PreemptGuard::depth() == 0,
                "IpcWaitQueue::wait: block_current() appelé avec PreemptGuard actif — deadlock garanti"
            );
            // SAFETY: waiter is active in table, wakeup is expected.
            unsafe {
                super::sched_hooks::block_current(tid);
            }
            spin_count += 1;
        }
    }

    // -----------------------------------------------------------------------
    // Wakeup
    // -----------------------------------------------------------------------

    /// Wakes up waiters according to configured policy.
    /// `reason`: reason provided to waiters.
    /// Returns the number of woken waiters.
    pub fn wake(&self, reason: WakeReason) -> u32 {
        let policy_raw = self.policy.load(Ordering::Relaxed);
        let policy = match policy_raw {
            1 => WakePolicy::All,
            2 => WakePolicy::UpToN,
            _ => WakePolicy::One,
        };
        self.wake_with_policy(reason, policy, u32::MAX)
    }

    /// Wakes up with explicit policy and limit N.
    pub fn wake_with_policy(&self, reason: WakeReason, policy: WakePolicy, max_n: u32) -> u32 {
        let mut woken = 0u32;
        for i in 0..MAX_IPC_WAITERS {
            match policy {
                WakePolicy::One if woken >= 1 => break,
                WakePolicy::UpToN if woken >= max_n => break,
                _ => {}
            }
            let w = &self.waiters[i];
            if w.is_active() && !w.is_woken() {
                let tid = w.thread_id.load(Ordering::Relaxed);
                if w.try_wake(reason) {
                    // Wake thread at scheduler level (if blocked).
                    super::sched_hooks::wake_thread(tid);
                    woken += 1;
                }
            }
        }
        self.total_woken.fetch_add(woken as u64, Ordering::Relaxed);
        woken
    }

    /// Wakes up a specific waiter by its thread_id.
    pub fn wake_thread(&self, thread_id: u64, reason: WakeReason) -> bool {
        for i in 0..MAX_IPC_WAITERS {
            let w = &self.waiters[i];
            if w.is_active() && w.thread_id.load(Ordering::Relaxed) == thread_id && !w.is_woken() {
                if w.try_wake(reason) {
                    super::sched_hooks::wake_thread(thread_id);
                    self.total_woken.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            }
        }
        false
    }

    /// Wakes all waiters with `Closed` (e.g. channel closed).
    pub fn close_all(&self) {
        self.wake_with_policy(WakeReason::Closed, WakePolicy::All, u32::MAX);
    }
}

// ---------------------------------------------------------------------------
// Statistics snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct IpcWaitQueueStats {
    pub total_waits: u64,
    pub total_woken: u64,
    pub total_timeouts: u64,
    pub current_waiters: u32,
}

impl IpcWaitQueue {
    pub fn snapshot_stats(&self) -> IpcWaitQueueStats {
        IpcWaitQueueStats {
            total_waits: self.total_waits.load(Ordering::Relaxed),
            total_woken: self.total_woken.load(Ordering::Relaxed),
            total_timeouts: self.total_timeouts.load(Ordering::Relaxed),
            current_waiters: self.waiter_count(),
        }
    }
}
