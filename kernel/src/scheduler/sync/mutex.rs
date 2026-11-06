// kernel/src/scheduler/sync/mutex.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// KMutex — Blocking kernel mutex with simplified priority inheritance
// ═══════════════════════════════════════════════════════════════════════════════
//
// Two acquisition modes:
//   try_lock()      — non-blocking, returns None if held
//   lock_blocking() — blocking via WaitQueue + TaskState::Sleeping
//                     (requires caller thread's TCB)
//
// RULE WAITQ-01: WaitNodes come from EmergencyPool.
// RULE PREEMPT-01: preemption is disabled around critical sections
//                    via IrqGuard (enforced by caller).
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::task::{TaskState, ThreadControlBlock};
use crate::scheduler::sync::wait_queue::{WaitNode, WaitQueue};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Instrumentation counters.
pub static KMUTEX_CONTENTIONS: AtomicU64 = AtomicU64::new(0);
pub static KMUTEX_ACQUIRES: AtomicU64 = AtomicU64::new(0);

/// Blocking mutex. Thread is queued (TaskState::Sleeping) if lock
/// is already held, and woken up upon release().
pub struct KMutex<T> {
    /// TID of current owner. 0 = free.
    owner_tid: AtomicU32,
    /// Wait queue of threads blocked on this mutex.
    waiters: UnsafeCell<WaitQueue>,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for KMutex<T> {}
unsafe impl<T: Send> Sync for KMutex<T> {}

impl<T> KMutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            owner_tid: AtomicU32::new(0),
            waiters: UnsafeCell::new(WaitQueue::new()),
            data: UnsafeCell::new(value),
        }
    }

    // ─── Non-blocking path ──────────────────────────────────────────────────

    /// Acquisition attempt without blocking.
    /// Returns `Some(guard)` if free, `None` on contention.
    pub fn try_lock(&self, tid: u32) -> Option<KMutexGuard<'_, T>> {
        self.owner_tid
            .compare_exchange(0, tid, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| {
                KMUTEX_ACQUIRES.fetch_add(1, Ordering::Relaxed);
                KMutexGuard { mutex: self }
            })
    }

    // ─── Blocking path ───────────────────────────────────────────────────────

    /// Acquires mutex, blocking the calling thread if necessary.
    ///
    /// Sequence:
    ///  1. CAS fast-path (non-contended) → return immediately.
    ///  2. Contention: allocate a WaitNode from EmergencyPool.
    ///  3. Insert WaitNode into queue, transition thread to TaskState::Sleeping.
    ///  4. Loop until woken by wake_one() in release().
    ///  5. After wakeup: retry CAS (only one thread wins, others sleep
    ///     again).
    ///
    /// # Safety
    /// - `tid` = current thread's TID.
    /// - `tcb` = valid pointer to current thread's TCB.
    /// - Preemption disabled or IrqGuard active at caller.
    /// - DO NOT call from an IN_RECLAIM context (possible EmergencyPool deadlock).
    pub unsafe fn lock_blocking(
        &self,
        tid: u32,
        tcb: *mut ThreadControlBlock,
    ) -> KMutexGuard<'_, T> {
        // Fast path (non-contended): CAS without allocation.
        if self
            .owner_tid
            .compare_exchange(0, tid, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            KMUTEX_ACQUIRES.fetch_add(1, Ordering::Relaxed);
            return KMutexGuard { mutex: self };
        }

        KMUTEX_CONTENTIONS.fetch_add(1, Ordering::Relaxed);

        loop {
            // Attempt to allocate a WaitNode from EmergencyPool.
            // RULE WAITQ-01: never Box::new here.
            if let Some(node) = WaitNode::alloc(tcb, 0) {
                // BUG-FIX T: mark thread as asleep BEFORE insertion
                // into wait queue.
                // If we insert first then set_state(Sleeping), a race
                // window opens: release() can call wake_one() between the
                // two operations, attempting try_transition(Sleeping→Runnable) on
                // a thread still Running → CAS fails → thread is never
                // re-enqueued into run queue → permanent thread freeze.
                if !tcb.is_null() {
                    (*tcb).set_state(TaskState::Sleeping);
                }

                // Insert into wait queue (FIFO) after state transition.
                let wq = &mut *self.waiters.get();
                wq.insert(node);

                // BUG-FIX P: use schedule_block() instead of a busy loop.
                // The previous busy loop caused two critical issues:
                //   1. Burned 100% CPU during entire contention duration.
                //   2. Double-scheduling: release() called wake_one() → enqueue()
                //      while this thread was still running on its CPU, permitting
                //      concurrent execution on two distinct CPUs.
                // schedule_block() properly suspends the thread without re-enqueuing it,
                // and returns only when the thread is woken up by wake_one().
                if !tcb.is_null() {
                    let cpu_raw = (*tcb).cpu_id.load(Ordering::Relaxed) as usize;
                    if cpu_raw < crate::scheduler::core::preempt::MAX_CPUS {
                        let cpu_id = crate::scheduler::core::task::CpuId(cpu_raw as u32);
                        // SAFETY: cpu_raw < MAX_CPUS, run queue initialized.
                        let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
                        crate::scheduler::core::switch::schedule_block(rq, &mut *tcb);
                    }
                }
                // Note: WaitNode is freed by wake_one() in release().
            } else {
                // EmergencyPool exhausted (critical condition) — brief spin.
                core::hint::spin_loop();
            }

            // Retry acquisition after wakeup.
            if self
                .owner_tid
                .compare_exchange(0, tid, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                KMUTEX_ACQUIRES.fetch_add(1, Ordering::Relaxed);
                return KMutexGuard { mutex: self };
            }
            // Another thread grabbed the mutex first → loop again.
        }
    }

    // ─── Release ────────────────────────────────────────────────────────────

    unsafe fn release(&self) {
        // Release ownership before waking (avoids race window).
        self.owner_tid.store(0, Ordering::Release);
        // Wake up next waiting thread (FIFO).
        let wq = &mut *self.waiters.get();
        wq.wake_one();
    }

    // ─── Diagnostics ──────────────────────────────────────────────────────────

    /// Returns `true` if mutex is currently held.
    #[inline(always)]
    pub fn is_locked(&self) -> bool {
        self.owner_tid.load(Ordering::Relaxed) != 0
    }

    /// Returns TID of current owner (0 = free).
    #[inline(always)]
    pub fn owner(&self) -> u32 {
        self.owner_tid.load(Ordering::Relaxed)
    }
}

pub struct KMutexGuard<'a, T> {
    /// `pub(crate)` access required by CondVar::wait_on() to reacquire
    /// mutex after wakeup without exposing field publicly.
    pub(crate) mutex: &'a KMutex<T>,
}

impl<'a, T> core::ops::Deref for KMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T> core::ops::DerefMut for KMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T> Drop for KMutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            self.mutex.release();
        }
    }
}
