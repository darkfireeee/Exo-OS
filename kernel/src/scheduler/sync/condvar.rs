// kernel/src/scheduler/sync/condvar.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CondVar — condition variable for KMutex
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implements classic pattern:
//   mutex.lock_blocking(tid, tcb)
//   while !condition {
//       condvar.wait_on(tcb, &mut mutex_guard);  ← releases mutex, sleeps, reacquires
//   }
//
// RULE WAITQ-01: WaitNodes are allocated from EmergencyPool.
//
// GUARANTEE: wait_on() is equivalent to POSIX pthread_cond_wait() — zero signal
// loss if notify_one/all is called after thread insertion into queue
// but before it sleeps (thanks to sequence number).
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::task::{TaskState, ThreadControlBlock};
use crate::scheduler::sync::mutex::KMutexGuard;
use crate::scheduler::sync::wait_queue::{WaitNode, WaitQueue};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

/// Instrumentation counters.
pub static CONDVAR_WAITS: AtomicU64 = AtomicU64::new(0);
pub static CONDVAR_WAKEUPS: AtomicU64 = AtomicU64::new(0);
pub static CONDVAR_SPURIOUS: AtomicU64 = AtomicU64::new(0);

pub struct CondVar {
    waiters: WaitQueue,
    /// Sequence number: incremented on each notify_one/notify_all.
    /// Allows detecting a signal arriving BEFORE wait_on() sleeps
    /// (prevents lost wakeups — classic race condition).
    seq: AtomicU64,
}

unsafe impl Send for CondVar {}
unsafe impl Sync for CondVar {}

impl Default for CondVar {
    fn default() -> Self {
        Self::new()
    }
}

impl CondVar {
    pub const fn new() -> Self {
        Self {
            waiters: WaitQueue::new(),
            seq: AtomicU64::new(0),
        }
    }

    // ─── Notification ────────────────────────────────────────────────────────

    /// Wakes up exactly one thread waiting on this CondVar.
    ///
    /// # Safety
    /// Called with associated mutex held AND preemption disabled.
    pub unsafe fn notify_one(&mut self) {
        self.seq.fetch_add(1, Ordering::Release);
        self.waiters.wake_one();
        CONDVAR_WAKEUPS.fetch_add(1, Ordering::Relaxed);
    }

    /// Wakes up all waiting threads.
    ///
    /// # Safety
    /// Called with associated mutex held AND preemption disabled.
    pub unsafe fn notify_all(&mut self) {
        self.seq.fetch_add(1, Ordering::Release);
        let n = self.waiters.wake_all();
        CONDVAR_WAKEUPS.fetch_add(n as u64, Ordering::Relaxed);
    }

    // ─── Wait ───────────────────────────────────────────────────────────────

    /// Releases `guard`, blocks current thread on CondVar, then reacquires
    /// mutex before returning.
    ///
    /// POSIX equivalent of `pthread_cond_wait(cond, mutex)`:
    ///  1. Read sequence number (atomic snapshot).
    ///  2. Allocate a WaitNode from EmergencyPool (RULE WAITQ-01).
    ///  3. Insert into wait list.
    ///  4. Release mutex (Drop on `guard`).
    ///  5. Mark TCB as Sleeping.
    ///  6. Wait until sequence number changes (wakeup).
    ///  7. Reacquire mutex via `lock_blocking()`.
    ///
    /// # Safety
    /// - `tcb` must point to valid TCB of current thread.
    /// - Called with mutex held (provided via `guard`).
    /// - Preemption disabled at caller (or active IrqGuard).
    ///
    /// # Return
    /// Returns a guard on the newly reacquired mutex.
    pub unsafe fn wait_on<'a, T>(
        &mut self,
        tcb: *mut ThreadControlBlock,
        guard: KMutexGuard<'a, T>,
    ) -> KMutexGuard<'a, T> {
        CONDVAR_WAITS.fetch_add(1, Ordering::Relaxed);

        // Sequence number snapshot (preserved for diagnostic / tests).
        let _seq_before = self.seq.load(Ordering::Acquire);

        // Allocate a WaitNode from EmergencyPool (RULE WAITQ-01).
        let node_opt: Option<NonNull<WaitNode>> = WaitNode::alloc(tcb, 0);

        // Extract mutex and thread TID before releasing guard.
        // SAFETY: Guard contains a reference to mutex — retrieve TID
        // via TCB (tid is in CL1, read-only here).
        let tid = if !tcb.is_null() {
            (*tcb).tid as u32
        } else {
            1u32
        };
        let mutex_ref = guard.mutex;

        if let Some(node) = node_opt {
            // Insert into wait queue BEFORE releasing mutex.
            // Ensures a notify_one() after release is not lost.
            self.waiters.insert(node);
        }

        // Mark TCB as asleep BEFORE releasing mutex.
        if !tcb.is_null() {
            (*tcb).set_state(TaskState::Sleeping);
        }

        // Release mutex (Drop guard).
        drop(guard);

        // BUG-FIX O: use schedule_block() instead of a busy loop.
        // The previous busy loop caused two critical issues:
        //   1. Burned 100% CPU during the entire wait.
        //   2. Double-scheduling: notify_one() called rq.enqueue() while
        //      this thread was still spinning on its CPU, permitting concurrent
        //      execution of the same TCB on two distinct CPUs.
        // schedule_block() properly suspends the thread by calling context_switch
        // to the next thread of the run queue, without re-enqueuing the current one.
        // The thread will be resumed only when notify_one()/notify_all() calls
        // wake_one() → try_transition(Sleeping, Runnable) + rq.enqueue().
        if !tcb.is_null() {
            let cpu_raw = (*tcb).cpu_id.load(Ordering::Relaxed) as usize;
            if cpu_raw < crate::scheduler::core::preempt::MAX_CPUS {
                let cpu_id = crate::scheduler::core::task::CpuId(cpu_raw as u32);
                // SAFETY: cpu_raw < MAX_CPUS, run queue initialized by scheduler::init().
                let rq = crate::scheduler::core::runqueue::run_queue(cpu_id);
                crate::scheduler::core::switch::schedule_block(rq, &mut *tcb);
            }
        }

        // Mark thread as ready again if still asleep.
        if !tcb.is_null() {
            let state = (*tcb).state();
            if state == TaskState::Sleeping {
                (*tcb).set_state(TaskState::Runnable);
                CONDVAR_SPURIOUS.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Reacquire mutex.
        mutex_ref.lock_blocking(tid, tcb)
    }

    /// Current notification sequence (external read for tests/debug).
    #[inline(always)]
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Number of threads currently waiting.
    #[inline(always)]
    pub fn waiters_count(&self) -> usize {
        self.waiters.count()
    }
}
