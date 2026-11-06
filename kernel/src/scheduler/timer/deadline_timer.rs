// kernel/src/scheduler/timer/deadline_timer.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Deadline Timer — EDF scheduling based on absolute deadlines
// ═══════════════════════════════════════════════════════════════════════════════
//
// Manages SCHED_DEADLINE thread queue sorted by increasing absolute deadline.
// Integrates deadline miss detection and reprogramming of the next period.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::task::ThreadControlBlock;
use crate::scheduler::policies::deadline::{check_deadline_miss, refresh_deadline};
use crate::scheduler::smp::topology::MAX_CPUS;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Per-CPU EDF queue — array sorted by increasing deadline_abs_ns
// ─────────────────────────────────────────────────────────────────────────────

const MAX_DL_TASKS: usize = 64;

struct DeadlineQueue {
    tasks: [Option<NonNull<ThreadControlBlock>>; MAX_DL_TASKS],
    count: usize,
}

unsafe impl Send for DeadlineQueue {}
unsafe impl Sync for DeadlineQueue {}

impl DeadlineQueue {
    const fn new() -> Self {
        Self {
            tasks: [None; MAX_DL_TASKS],
            count: 0,
        }
    }

    fn insert(&mut self, tcb: NonNull<ThreadControlBlock>) -> bool {
        if self.count >= MAX_DL_TASKS {
            return false;
        }
        let dl = unsafe {
            tcb.as_ref()
                .deadline_abs
                .load(core::sync::atomic::Ordering::Relaxed)
        };
        // Insertion sorted by increasing deadline_abs_ns.
        let mut pos = self.count;
        for i in 0..self.count {
            let d = unsafe {
                self.tasks[i]
                    .unwrap_unchecked()
                    .as_ref()
                    .deadline_abs
                    .load(core::sync::atomic::Ordering::Relaxed)
            };
            if d > dl {
                pos = i;
                break;
            }
        }
        let mut j = self.count;
        while j > pos {
            self.tasks[j] = self.tasks[j - 1];
            j -= 1;
        }
        self.tasks[pos] = Some(tcb);
        self.count += 1;
        true
    }

    /// Extracts thread with earliest deadline.
    fn pop_earliest(&mut self) -> Option<NonNull<ThreadControlBlock>> {
        if self.count == 0 {
            return None;
        }
        let tcb = self.tasks[0];
        let mut j = 0;
        while j + 1 < self.count {
            self.tasks[j] = self.tasks[j + 1];
            j += 1;
        }
        self.tasks[self.count - 1] = None;
        self.count -= 1;
        tcb
    }

    /// Returns (without extracting) thread with earliest deadline.
    #[allow(dead_code)]
    fn peek_earliest(&self) -> Option<NonNull<ThreadControlBlock>> {
        self.tasks[0]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global instances
// ─────────────────────────────────────────────────────────────────────────────

use core::mem::MaybeUninit;

static mut DL_QUEUES: [MaybeUninit<DeadlineQueue>; MAX_CPUS] =
    [const { MaybeUninit::uninit() }; MAX_CPUS];

pub static DL_ENQUEUES: AtomicU64 = AtomicU64::new(0);
pub static DL_DEQUEUES: AtomicU64 = AtomicU64::new(0);
pub static DL_MISS_EVENTS: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes deadline queues.
///
/// # Safety
/// Called once from scheduler::init().
pub unsafe fn init(nr_cpus: usize) {
    for cpu in 0..nr_cpus.min(MAX_CPUS) {
        DL_QUEUES[cpu].write(DeadlineQueue::new());
    }
}

/// Enqueues a SCHED_DEADLINE thread into EDF queue of CPU `cpu`.
///
/// Refreshes absolute deadline via `refresh_deadline()` prior to insertion.
///
/// # Safety
/// Requires preemption disabled.
pub unsafe fn dl_enqueue(cpu: usize, tcb_ptr: NonNull<ThreadControlBlock>) -> bool {
    let tcb = &mut *tcb_ptr.as_ptr();
    refresh_deadline(tcb);
    let q = DL_QUEUES[cpu].assume_init_mut();
    if q.insert(tcb_ptr) {
        DL_ENQUEUES.fetch_add(1, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Extracts highest priority EDF thread (earliest deadline).
///
/// # Safety
/// Requires preemption disabled.
pub unsafe fn dl_pick_next(cpu: usize) -> Option<NonNull<ThreadControlBlock>> {
    let q = DL_QUEUES[cpu].assume_init_mut();
    let tcb_opt = q.pop_earliest();
    if let Some(tcb) = tcb_opt {
        tcb.as_ref().clear_queued();
        DL_DEQUEUES.fetch_add(1, Ordering::Relaxed);
    }
    tcb_opt
}

/// Removes a specific SCHED_DEADLINE thread from EDF queue of CPU `cpu`.
/// Used during thread migration or termination.
///
/// # Safety
/// Requires preemption disabled.
pub unsafe fn dl_remove(cpu: usize, target: NonNull<ThreadControlBlock>) -> bool {
    let q = DL_QUEUES[cpu].assume_init_mut();
    for i in 0..q.count {
        if q.tasks[i] == Some(target) {
            let mut j = i;
            while j + 1 < q.count {
                q.tasks[j] = q.tasks[j + 1];
                j += 1;
            }
            q.tasks[q.count - 1] = None;
            q.count -= 1;
            target.as_ref().clear_queued();
            return true;
        }
    }
    false
}

/// Checks for deadline misses on CPU `cpu` on every tick.
///
/// # Safety
/// Requires preemption disabled.
pub unsafe fn dl_tick(cpu: usize) {
    let q = DL_QUEUES[cpu].assume_init_mut();
    for i in 0..q.count {
        if let Some(tcb_ptr) = q.tasks[i] {
            let tcb = tcb_ptr.as_ref();
            if check_deadline_miss(tcb) {
                DL_MISS_EVENTS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
