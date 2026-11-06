// kernel/src/scheduler/policies/deadline.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// SCHED_DEADLINE policy — EDF (Earliest Deadline First)
// ═══════════════════════════════════════════════════════════════════════════════
//
// SCHED_DEADLINE is a strict real-time policy based on EDF.
// Each configured thread specifies:
//   runtime_ns  — execution budget per period
//   deadline_ns — relative deadline from activation
//   period_ns   — reactivation period
//
// Admission test: Σ(runtime_i / period_i) ≤ 1.0
// If test fails, `admit_thread()` returns `Err(AdmissionDenied)`.
//
// Schedulability: on each wakeup, `deadline_abs = now + deadline_ns`.
// The EDF run queue sorts by increasing `deadline_abs`.
// ═══════════════════════════════════════════════════════════════════════════════

use crate::scheduler::core::task::{DeadlineParams, SchedPolicy, ThreadControlBlock};
use crate::scheduler::timer::clock::monotonic_ns;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Admission accounting
// ─────────────────────────────────────────────────────────────────────────────

/// Numerator of utilized fraction = Σ runtime_i × SCALE / period_i.
/// SCALE = 1_000_000 to avoid floating point math.
const ADMIT_SCALE: u64 = 1_000_000;

/// Cumulative utilization × SCALE (max = ADMIT_SCALE = 100%).
static ADMITTED_UTILIZATION: AtomicU64 = AtomicU64::new(0);
/// Number of admitted DEADLINE threads.
pub static DEADLINE_THREADS_ADMITTED: AtomicU64 = AtomicU64::new(0);
/// Number of admission denials.
pub static DEADLINE_ADMISSION_DENIED: AtomicU64 = AtomicU64::new(0);
/// Detected deadline misses.
pub static DEADLINE_MISSES: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Admission error
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineError {
    /// Admission test failed (overload).
    AdmissionDenied,
    /// Invalid parameters (period=0, deadline>period, runtime>deadline).
    InvalidParams,
    /// Lack of capacity in EDF run queue.
    QueueFull,
}

// ─────────────────────────────────────────────────────────────────────────────
// Parameter validation
// ─────────────────────────────────────────────────────────────────────────────

fn validate_params(p: &DeadlineParams) -> Result<(), DeadlineError> {
    if p.period_ns == 0 {
        return Err(DeadlineError::InvalidParams);
    }
    if p.deadline_ns > p.period_ns {
        return Err(DeadlineError::InvalidParams);
    }
    if p.runtime_ns == 0 || p.runtime_ns > p.deadline_ns {
        return Err(DeadlineError::InvalidParams);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Admission test
// ─────────────────────────────────────────────────────────────────────────────

/// Attempts to admit a new DEADLINE thread.
///
/// Performs `Σ(runtime/period) ≤ 1.0` via CAS on `ADMITTED_UTILIZATION`.
/// Returns `Ok(fraction)` (fraction = runtime/period × SCALE) if admitted,
/// or `Err(AdmissionDenied)` if load exceeds capacity.
pub fn admit_thread(p: &DeadlineParams) -> Result<u64, DeadlineError> {
    validate_params(p)?;

    // fraction = runtime_ns × SCALE / period_ns
    let fraction = p.runtime_ns.saturating_mul(ADMIT_SCALE) / p.period_ns;

    // CAS loop for atomic update.
    loop {
        let current = ADMITTED_UTILIZATION.load(Ordering::Acquire);
        let new_util = current.saturating_add(fraction);
        if new_util > ADMIT_SCALE {
            DEADLINE_ADMISSION_DENIED.fetch_add(1, Ordering::Relaxed);
            return Err(DeadlineError::AdmissionDenied);
        }
        if ADMITTED_UTILIZATION
            .compare_exchange(current, new_util, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            DEADLINE_THREADS_ADMITTED.fetch_add(1, Ordering::Relaxed);
            return Ok(fraction);
        }
        // CAS contention — retry.
        core::hint::spin_loop();
    }
}

/// Releases utilization fraction upon thread termination/descheduling.
///
/// # BUG-FIX E: use fetch_update + saturating_sub to avoid silent underflow
/// on u64. Previous `fetch_sub` could cause ADMITTED_UTILIZATION and
/// DEADLINE_THREADS_ADMITTED to wrap to u64::MAX≈1.8×10¹⁹ if
/// `release_thread()` was called excessively (double-free bug).
pub fn release_thread(fraction: u64) {
    let _ = ADMITTED_UTILIZATION.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
        Some(v.saturating_sub(fraction))
    });
    let _ = DEADLINE_THREADS_ADMITTED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
        Some(v.saturating_sub(1))
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Absolute deadline management
// ─────────────────────────────────────────────────────────────────────────────

/// Recalculates `deadline_abs` upon DEADLINE thread wakeup.
///
/// EDF: upon activation, deadline_abs = now + deadline_params.deadline_ns
pub fn refresh_deadline(tcb: &mut ThreadControlBlock) {
    let now = monotonic_ns();
    let deadline_ns = tcb.dl_period;
    tcb.deadline_abs
        .store(now.saturating_add(deadline_ns), Ordering::Release);
}

/// Checks if thread missed its deadline.
/// Increments `DEADLINE_MISSES` and returns `true` if so.
pub fn check_deadline_miss(tcb: &ThreadControlBlock) -> bool {
    let now = monotonic_ns();
    let deadline_abs = tcb.deadline_abs.load(Ordering::Relaxed);
    if now > deadline_abs && deadline_abs != 0 {
        DEADLINE_MISSES.fetch_add(1, Ordering::Relaxed);
        return true;
    }
    false
}

/// Returns remaining time (in ns) before thread absolute deadline.
/// Returns 0 if deadline already passed or uninitialized.
pub fn remaining_budget(tcb: &ThreadControlBlock) -> u64 {
    let now = monotonic_ns();
    let deadline_abs = tcb.deadline_abs.load(Ordering::Relaxed);
    if deadline_abs == 0 || now >= deadline_abs {
        0
    } else {
        deadline_abs - now
    }
}

/// Called during timer tick for a DEADLINE thread.
/// Decrements remaining budget, returns `true` if budget is exhausted
/// (thread must block until its next period).
pub fn deadline_tick(tcb: &ThreadControlBlock, elapsed_ns: u64) -> bool {
    if tcb.policy != SchedPolicy::Deadline {
        return false;
    }
    let budget = remaining_budget(tcb);
    elapsed_ns >= budget
}
