// kernel/src/scheduler/policies/cfs.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CFS — Completely Fair Scheduler (Exo-OS Scheduler · Layer 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// CFS implements proportional scheduling: each thread receives a share
// of the CPU proportional to its weight (based on nice/priority).
//
// Core invariant: the thread with the smallest `vruntime` is always
// picked first. vruntime is weighted to compensate for priority
// differences (higher priority threads advance more slowly).
//
// This module provides CFS decision logic; the run queue (min-heap sorted
// by vruntime) resides in core/runqueue.rs.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::runqueue::{CFS_MIN_GRANULARITY_US, CFS_TARGET_LATENCY_MS};
use crate::scheduler::core::task::{SchedPolicy, ThreadControlBlock};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// CFS constants
// ─────────────────────────────────────────────────────────────────────────────

/// Minimum slice guaranteed to each thread even with a deep queue (ns).
pub const CFS_MIN_SLICE_NS: u64 = CFS_MIN_GRANULARITY_US * 1000;
/// Target latency period (ns) — time window within which every thread must run.
pub const CFS_TARGET_PERIOD_NS: u64 = CFS_TARGET_LATENCY_MS * 1_000_000;
/// Wakeup preemption threshold: if outgoing thread advanced its vruntime by
/// more than this threshold relative to min_vruntime, preempt immediately.
pub const CFS_WAKEUP_PREEMPT_NS: u64 = 1_000_000; // 1ms

const _: () = assert!(
    CFS_MIN_SLICE_NS < CFS_TARGET_PERIOD_NS,
    "CFS minimum slice must fit inside the target period"
);
const _: () = assert!(
    CFS_WAKEUP_PREEMPT_NS < CFS_TARGET_PERIOD_NS,
    "CFS wakeup preemption threshold must fit inside the target period"
);

/// Global CFS preemption counter (instrumentation).
pub static CFS_PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
/// Global wakeup-preemption counter (instrumentation).
/// BUG-FIX I: typo fix (PREMPT → PREEMPT).
pub static CFS_WAKEUP_PREEMPT_COUNT: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// CFS logic
// ─────────────────────────────────────────────────────────────────────────────

/// Computes timeslice allocated to `tcb` according to number of threads in run queue.
///
/// Linux CFS formula:
///   slice = max(target_period × weight / total_weight, min_granularity)
///
/// With `nr_tasks` threads of equal weight:
///   slice = target_period / nr_tasks  (≥ min_granularity)
pub fn timeslice_for(tcb: &ThreadControlBlock, nr_tasks: usize, total_weight: u64) -> u64 {
    if nr_tasks == 0 {
        return CFS_TARGET_PERIOD_NS;
    }
    let nr = nr_tasks.max(1);
    let weight = tcb.priority.cfs_weight() as u64;
    let raw_slice = CFS_TARGET_PERIOD_NS
        .saturating_mul(weight)
        .checked_div(total_weight)
        .unwrap_or(CFS_TARGET_PERIOD_NS / nr as u64);
    raw_slice.max(CFS_MIN_SLICE_NS)
}

/// Checks if current thread should be preempted in favor of a lighter thread
/// (with smaller vruntime) that just woke up.
///
/// RULE: called in `wakeup_thread()` after reinsertion into CFS queue.
/// Returns `true` if preemption is desirable.
pub fn should_preempt_on_wakeup(
    running: &ThreadControlBlock,
    woken: &ThreadControlBlock,
    _min_vruntime: u64,
) -> bool {
    // Only preempt if current thread is not RT.
    if running.priority.is_realtime() {
        return false;
    }

    let running_vr = running.vruntime.load(Ordering::Relaxed);
    let woken_vr = woken.vruntime.load(Ordering::Relaxed);

    // FIX-VRUNTIME-01: use wrapping_add to avoid debug panic
    // and silent release wrap. Correct semantics: preempt if
    // woken thread has run significantly LESS than current thread.
    let woken_vr_bumped = woken_vr.wrapping_add(CFS_WAKEUP_PREEMPT_NS);
    if woken_vr_bumped < running_vr {
        CFS_WAKEUP_PREEMPT_COUNT.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Normalizes vruntime of a newly enqueued thread to prevent it
/// from monopolizing CPU by always returning with a very low vruntime.
///
/// CFS rule: place new thread at `max(own_vr, min_vruntime - timeslice/2)`.
pub fn normalize_vruntime_on_enqueue(tcb: &ThreadControlBlock, min_vruntime: u64, slice_ns: u64) {
    let current_vr = tcb.vruntime.load(Ordering::Relaxed);
    let floor = min_vruntime.saturating_sub(slice_ns / 2);
    if current_vr < floor {
        // SAFETY: we are the sole producer — run queue owns the TCB.
        tcb.vruntime.store(floor, Ordering::Relaxed);
    }
}

/// Called by timer tick to check if current thread has exhausted its timeslice.
///
/// Returns `true` if thread should be preempted.
pub fn tick_check_preempt(
    tcb: &ThreadControlBlock,
    elapsed_ns: u64,
    slice_ns: u64,
    nr_tasks: usize,
) -> bool {
    if tcb.policy != SchedPolicy::Normal && tcb.policy != SchedPolicy::Batch {
        return false;
    }
    if nr_tasks <= 1 {
        // Sole thread → no preemption necessary.
        return false;
    }
    if elapsed_ns >= slice_ns {
        CFS_PREEMPTIONS.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}
