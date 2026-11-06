// kernel/src/scheduler/core/pick_next.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PICK_NEXT_TASK — O(1) next thread selection (Exo-OS Scheduler · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// PERFORMANCE OBJECTIVE (DOC3):
//   • pick_next_task() targets 100-150 cycles in hot path
//   • Priority order is strictly respected: RT > CFS > Idle
//   • AI hints (ai_guided.rs) may only SUGGEST, never override RT
//
// RULES:
//   • NO-ALLOC — no allocation in this function
//   • Called with preemption disabled (debug check)
//   • Complete instrumentation (atomic counters)
//   • SMP-compatible: each CPU calls on ITS own run queue
// ═══════════════════════════════════════════════════════════════════════════════

use super::preempt::assert_preempt_disabled;
use super::runqueue::{PerCpuRunQueue, MAX_TASKS_PER_CPU};
use super::task::{TaskState, ThreadControlBlock};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Global instrumentation counters
// ─────────────────────────────────────────────────────────────────────────────

/// Total pick_next_task() invocations since boot.
pub static PICK_NEXT_TOTAL: AtomicU64 = AtomicU64::new(0);
/// Number of times the current thread was retained without switching.
pub static PICK_SAME_CURRENT: AtomicU64 = AtomicU64::new(0);
/// Number of RT -> RT switches.
pub static PICK_RT_RT: AtomicU64 = AtomicU64::new(0);
/// Number of times an ineligible thread was skipped.
pub static PICK_SKIP_INELIGIBLE: AtomicU64 = AtomicU64::new(0);

const MAX_PICK_SKIP_SCAN: usize = MAX_TASKS_PER_CPU + 256 + 32;

// ─────────────────────────────────────────────────────────────────────────────
// pick_next result
// ─────────────────────────────────────────────────────────────────────────────

/// Scheduling decision for this tick.
pub enum PickResult {
    /// No change — current thread continues.
    KeepRunning,
    /// Switch to this new thread.
    Switch(NonNull<ThreadControlBlock>),
    /// Enter idle (no thread to execute).
    GoIdle,
}

// ─────────────────────────────────────────────────────────────────────────────
// pick_next_task() — core function
// ─────────────────────────────────────────────────────────────────────────────

/// Selects the next thread to run on the current CPU.
///
/// # Preconditions
/// - Preemption disabled on the calling CPU.
/// - The run queue is that of the current CPU.
///
/// # Algorithm
/// 1. If RT queue not empty -> dequeue highest priority RT thread.
/// 2. Else -> dequeue CFS head (minimum vruntime).
/// 3. Else -> return idle thread.
///
/// The AI hint is consulted ONLY for CFS tiebreaking (no RT impact).
///
/// # Performance guarantee
/// In the absence of contention, this function executes in 100-150 cycles.
///
/// # Safety
/// Preemption must be disabled on the current CPU. `rq` and `current` must be valid.
#[inline(always)]
pub unsafe fn pick_next_task(
    rq: &mut PerCpuRunQueue,
    current: Option<NonNull<ThreadControlBlock>>,
) -> PickResult {
    assert_preempt_disabled();

    PICK_NEXT_TOTAL.fetch_add(1, Ordering::Relaxed);

    // Fast path: if the current thread is RT and still the highest priority,
    // do not switch (hot path optimization).
    if let Some(cur) = current {
        let cur_ref = cur.as_ref();
        if cur_ref.state() == TaskState::Running
            && (cur_ref.policy == crate::scheduler::core::task::SchedPolicy::Fifo
                || cur_ref.policy == crate::scheduler::core::task::SchedPolicy::RoundRobin)
        {
            // Check if a higher-priority RT thread is waiting.
            if let Some(best_prio) = rq.rt_bitmap_highest_prio() {
                if best_prio < cur_ref.priority.0 {
                    // Preempt current RT thread in favor of a higher-priority RT thread.
                    PICK_RT_RT.fetch_add(1, Ordering::Relaxed);
                    // Re-enqueue current: even FIFO must remain ready after
                    // preemption by a higher RT priority.
                    cur_ref.set_state(TaskState::Runnable);
                    rq.enqueue(cur);
                    if let Some(next) = rq.dequeue_highest_rt() {
                        return PickResult::Switch(next);
                    }
                    return PickResult::KeepRunning;
                }
            }
            // No higher priority RT -> current continues.
            PICK_SAME_CURRENT.fetch_add(1, Ordering::Relaxed);
            return PickResult::KeepRunning;
        }
    }

    // Standard selection. A runqueue may transiently contain a stale entry
    // after concurrent wakeup/preemption; pop it and continue
    // instead of falling back to idle when a runnable parent is waiting behind.
    for _ in 0..MAX_PICK_SKIP_SCAN {
        match rq.pick_next() {
            None => return PickResult::GoIdle,
            Some(next) => {
                // Check: only a Runnable thread can be selected.
                // Exception: current idle is published as Running and serves as
                // last resort when no active queue has work.
                let next_ref = next.as_ref();
                let state = next_ref.state();
                let idle_running = state == TaskState::Running && next_ref.is_idle();
                if state != TaskState::Runnable && !idle_running {
                    // Ineligible thread (zombie, stopped, etc.) — do not reinsert.
                    PICK_SKIP_INELIGIBLE.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                // Pure deterministic CFS selection — no heuristic adjustment.
                let final_next = next;

                if current == Some(final_next) {
                    PICK_SAME_CURRENT.fetch_add(1, Ordering::Relaxed);
                    return PickResult::KeepRunning;
                } else {
                    return PickResult::Switch(final_next);
                }
            }
        }
    }

    PickResult::GoIdle
}

// ─────────────────────────────────────────────────────────────────────────────
// Update vruntime after execution timeslice
// ─────────────────────────────────────────────────────────────────────────────

/// Accounts CPU time for thread `tcb` for `delta_ns` nanoseconds.
/// To be called in the tick handler, with preemption disabled.
///
/// # Safety
/// Preemption must be disabled on the current CPU. `tcb` must be valid.
#[inline(always)]
pub unsafe fn account_time(tcb: &ThreadControlBlock, delta_ns: u64) {
    let weight = tcb.priority.cfs_weight();
    match tcb.policy {
        crate::scheduler::core::task::SchedPolicy::Normal
        | crate::scheduler::core::task::SchedPolicy::Batch => {
            tcb.advance_vruntime(delta_ns, weight);
        }
        // RT: no vruntime — preemption based on fixed priority.
        _ => {}
    }
}
