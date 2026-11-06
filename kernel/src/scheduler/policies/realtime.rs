// kernel/src/scheduler/policies/realtime.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Real-Time Policy: SCHED_FIFO + SCHED_RR (Exo-OS · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// SCHED_FIFO: no preemption within the same priority.
//             An RT thread runs until it blocks or yields.
//             A higher priority RT thread can preempt.
//
// SCHED_RR:   identical to FIFO but each thread has a 10ms quantum.
//             Upon expiration, the thread is re-enqueued at the end of its
//             priority queue.
//
// RT Priorities: 1 (lowest) .. 99 (highest) per POSIX.
//   Internal: stored in Priority::rt_prio (0 = idle, 100–199 = RT 1–100)
//   RT RunQueue uses levels 100–199 for RT queues.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::runqueue::PerCpuRunQueue;
use crate::scheduler::core::task::{SchedPolicy, ThreadControlBlock};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// SCHED_RR quantum (10ms in nanoseconds).
pub const RR_TIMESLICE_NS: u64 = 10_000_000;
/// Lowest POSIX RT priority.
pub const RT_PRIO_MIN: u8 = 1;
/// Highest POSIX RT priority.
pub const RT_PRIO_MAX: u8 = 99;

const _: () = assert!(RT_PRIO_MIN < RT_PRIO_MAX, "RT priority range is invalid");
const _: () = assert!(
    (RT_PRIO_MAX as usize) < crate::scheduler::core::runqueue::RT_LEVELS,
    "RT runqueue does not cover POSIX RT priorities"
);

/// Counter of RT preemptions caused by higher priority thread interruption.
pub static RT_PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
/// RR quantum expirations.
pub static RR_QUANTUM_EXPIRATIONS: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// SCHED_FIFO logic
// ─────────────────────────────────────────────────────────────────────────────

/// Checks if a higher-priority RT thread is waiting in the run
/// queue, which would trigger preemption of current thread.
///
/// Returns `true` if preemption is required.
///
/// # BUG-FIX Q
/// Old code used `rt_highest_prio()` which returns 0 (= maximum
/// RT priority) even when the RT queue is EMPTY. Result: `highest_waiting (0)
/// < running_prio` was true for any non-RT thread (prio > 0), triggering
/// ghost preemption with no RT thread waiting.
/// Fix: `rt_bitmap_highest_prio()` returns `None` if queue is empty
/// — preemption is only triggered if a thread is genuinely waiting.
pub fn fifo_should_preempt(rq: &PerCpuRunQueue, running: &ThreadControlBlock) -> bool {
    let running_prio = running.priority.0;
    // rt_bitmap_highest_prio() returns None if no RT thread is ready.
    match rq.rt_bitmap_highest_prio() {
        Some(highest_waiting) if highest_waiting < running_prio => {
            RT_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
            true
        }
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SCHED_RR logic
// ─────────────────────────────────────────────────────────────────────────────

/// Called by timer tick for a SCHED_RR thread.
///
/// Returns `true` if quantum expired (thread must be re-enqueued at
/// the end of its priority queue).
pub fn rr_tick(tcb: &ThreadControlBlock, elapsed_since_schedule_ns: u64) -> bool {
    if tcb.policy != SchedPolicy::RoundRobin {
        return false;
    }
    if elapsed_since_schedule_ns >= RR_TIMESLICE_NS {
        RR_QUANTUM_EXPIRATIONS.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Remaining quantum for a SCHED_RR thread (in nanoseconds).
pub fn rr_remaining_slice(elapsed_since_schedule_ns: u64) -> u64 {
    RR_TIMESLICE_NS.saturating_sub(elapsed_since_schedule_ns)
}
