// kernel/src/arch/x86_64/time/drift/periodic.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Periodic TSC drift recalibration thread
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Role
//   Periodically measures TSC drift by comparing TSC and HPET (or PM Timer)
//   over a real time window, then corrects the frequency in KtimeState
//   via the PLL to avoid abrupt jumps.
//
// ## Periodicity
//   - Idle (CPU load < 20%) : every 30 seconds
//   - Normal                : every 10 seconds
//   - Heavy load (> 80%)    : every 5 seconds (greater thermal drift)
//
// ## Critical rules
//   RULE DRIFT-PREEMPT-01  : preempt_disable() BEFORE HPET+TSC measurement
//                            -> prevents thread from being preempted between the two reads
//   RULE DRIFT-CIRCULAR-01 : Do NOT call ktime_get_ns() here
//                            -> read HPET and TSC directly via low-level functions
//   RULE DRIFT-PLL-01      : correction <= ±500 ppm per cycle (via pll.rs)
//   RULE DRIFT-MONOTONE-01 : ns_anchor_new >= ns_anchor_old (check before write)
//
// ## Architecture
//   This module is called periodically from the tick handler (tick.rs).
//   It does NOT maintain a dedicated thread (too costly in Phase 2b).
//   Long term (Phase 4+): dedicated kthread with HLT/MWAIT in idle.
// ════════════════════════════════════════════════════════════════════════════════

use super::super::calibration::window;
use super::super::ktime;
use super::super::sources::{hpet as hpet_src, pm_timer as pm_src};
use super::pll;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ── Periodicity constants ─────────────────────────────────────────────────────

/// Recalibration interval under normal conditions (ns).
const RECAL_INTERVAL_NORMAL_NS: u64 = 10_000_000_000; // 10 seconds
/// Interval under heavy load (ns).
const RECAL_INTERVAL_HEAVY_NS: u64 = 5_000_000_000; // 5 seconds
/// Interval in idle (ns).
const RECAL_INTERVAL_IDLE_NS: u64 = 30_000_000_000; // 30 seconds

/// CPU load threshold for "idle" (percentage * 10 = permille).
const IDLE_THRESHOLD_PERMIL: u32 = 200; // 20%
/// CPU load threshold for "heavy load".
const HEAVY_THRESHOLD_PERMIL: u32 = 800; // 80%

/// Measurement history size (circular FIFO for anomaly detection).
#[allow(dead_code)]
const HISTORY_SIZE: usize = 8;

// ── Recalibration thread state ────────────────────────────────────────────────

struct DriftState {
    /// Timestamp (ktime ns) of last recalibration.
    last_recal_ns: AtomicU64,
    /// Current recalibration interval (adaptive).
    interval_ns: AtomicU64,
    /// Total number of recalibrations performed since boot.
    recal_count: AtomicU64,
    /// Number of failed recalibrations (HPET/PM Timer unavailable or out-of-range measurement).
    fail_count: AtomicU64,
    /// Current CPU load in permille (0–1000), updated by tick handler.
    cpu_load_permil: AtomicU32,
    /// `true` if recalibration in progress (guard against SMP re-entrancy).
    in_progress: AtomicBool,
    /// Number of times monotonicity had to be fixed (excessively aggressive PLL drift).
    monotone_fixes: AtomicU64,
    /// Last measured TSC frequency (before PLL).
    last_measured_hz: AtomicU64,
    /// Last applied TSC frequency (after PLL).
    last_applied_hz: AtomicU64,
}

impl DriftState {
    const fn new() -> Self {
        Self {
            last_recal_ns: AtomicU64::new(0),
            interval_ns: AtomicU64::new(RECAL_INTERVAL_NORMAL_NS),
            recal_count: AtomicU64::new(0),
            fail_count: AtomicU64::new(0),
            cpu_load_permil: AtomicU32::new(0),
            in_progress: AtomicBool::new(false),
            monotone_fixes: AtomicU64::new(0),
            last_measured_hz: AtomicU64::new(0),
            last_applied_hz: AtomicU64::new(0),
        }
    }
}

static DRIFT: DriftState = DriftState::new();

// ── Initialization ────────────────────────────────────────────────────────────

/// Initializes the drift correction module.
/// Called from `time_init()` after initial calibration.
pub fn drift_init(initial_hz: u64) {
    pll::pll_init(initial_hz);
    DRIFT.last_applied_hz.store(initial_hz, Ordering::Release);
}

// ── Periodic entry point ──────────────────────────────────────────────────────

/// Checks whether recalibration is due and executes it if necessary.
///
/// Called from tick handler (tick.rs) on each tick (HZ=1000, i.e. 1ms).
/// Does NOT block — returns immediately if interval has not elapsed.
///
/// RULE DRIFT-CIRCULAR-01: This function reads TSC directly.
///   It does NOT call ktime_get_ns() to avoid circular dependency.
/// RULE DRIFT-PREEMPT-01: preempt_disable() before measurement window.
pub fn drift_tick(tsc_now_direct: u64) {
    // Read last recalibration timestamp.
    let last = DRIFT.last_recal_ns.load(Ordering::Relaxed);
    let interval = DRIFT.interval_ns.load(Ordering::Relaxed);

    // Calculate elapsed time since last recalibration.
    // RULE DRIFT-CIRCULAR-01: We use TSC directly + current PLL frequency,
    // not ktime_get_ns() which depends on tsc_hz being updated.
    let current_hz = pll::pll_current_hz();
    if current_hz == 0 {
        return;
    }

    // Read snapshot ONCE for consistency (avoids two disjoint atomic reads).
    let snap = ktime::ktime_snapshot();
    let elapsed_tsc = tsc_now_direct.wrapping_sub(snap.tsc_base);
    let elapsed_ns = (elapsed_tsc as u128 * 1_000_000_000 / current_hz as u128) as u64;

    // Estimated absolute time = ns_base + elapsed since last anchor.
    let estimated_ns = snap.ns_base.wrapping_add(elapsed_ns);

    if last > 0 && estimated_ns.wrapping_sub(last) < interval {
        return; // Not yet time to recalibrate.
    }

    // Guard against SMP re-entrancy: only one CPU recalibrates at a time.
    if DRIFT
        .in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    // Start recalibration.
    let result = do_recalibrate(tsc_now_direct, estimated_ns);

    // Adapt interval based on CPU load.
    adapt_interval();

    DRIFT.in_progress.store(false, Ordering::Release);

    if result {
        DRIFT.last_recal_ns.store(estimated_ns, Ordering::Release);
        DRIFT.recal_count.fetch_add(1, Ordering::Relaxed);
    } else {
        DRIFT.fail_count.fetch_add(1, Ordering::Relaxed);
    }
}

// ── Recalibration core ────────────────────────────────────────────────────────

/// Performs a drift measurement and applies PLL correction.
///
/// Returns `true` if a correction was applied, `false` if measurement failed.
///
/// RULE DRIFT-PREEMPT-01: preempt disabled during measurement window.
/// RULE DRIFT-CIRCULAR-01: HPET and TSC read directly.
fn do_recalibrate(tsc_anchor: u64, ns_anchor: u64) -> bool {
    // Attempt recalibration via HPET (preferred source).
    let measured_hz = if hpet_src::available() {
        let freq = hpet_src::freq_hz();
        // Short window (1ms) to limit preempt_disable duration.
        // Use direct version from window.rs (CAL-WINDOW-01 + CAL-CLI-01).
        match window::calibrate_tsc_via_hpet(freq) {
            Some(hz) if is_plausible_hz(hz) => hz,
            _ => {
                // PM Timer fallback.
                match try_pm_timer_measure() {
                    Some(hz) => hz,
                    None => return false,
                }
            }
        }
    } else if pm_src::available() {
        match try_pm_timer_measure() {
            Some(hz) => hz,
            None => return false,
        }
    } else {
        return false; // No reference source available.
    };

    DRIFT.last_measured_hz.store(measured_hz, Ordering::Relaxed);

    // Calculate current absolute time from tsc_anchor + offset.
    // RULE DRIFT-CIRCULAR-01: use tsc_anchor and ns_anchor passed as parameter,
    // not ktime_get_ns().
    let current_hz_pll = pll::pll_current_hz();
    let tsc_since_anchor = rdtsc_direct().wrapping_sub(tsc_anchor);
    let ns_since_anchor = if current_hz_pll > 0 {
        (tsc_since_anchor as u128 * 1_000_000_000 / current_hz_pll as u128) as u64
    } else {
        0
    };
    let ns_now = ns_anchor.wrapping_add(ns_since_anchor);

    // Apply PLL correction.
    let (ns_corrected, new_hz) = pll::pll_update(measured_hz, rdtsc_direct(), ns_now);

    // Strict monotonicity check.
    // RULE DRIFT-MONOTONE-01: ns_corrected >= current ktime::ns_base.
    let snap = ktime::ktime_snapshot();
    let ns_final = if ns_corrected < snap.ns_base {
        DRIFT.monotone_fixes.fetch_add(1, Ordering::Relaxed);
        snap.ns_base + 1 // Advance by at least 1 ns to maintain monotonicity.
    } else {
        ns_corrected
    };

    let tsc_final = rdtsc_direct();

    // Write new anchor to KtimeState (seqlock).
    ktime::update_ktime_anchor(tsc_final, ns_final, new_hz);

    DRIFT.last_applied_hz.store(new_hz, Ordering::Relaxed);
    true
}

/// Attempts a measurement via PM Timer (fallback when HPET absent).
fn try_pm_timer_measure() -> Option<u64> {
    match window::calibrate_tsc_via_pm_timer() {
        Some(hz) if is_plausible_hz(hz) => Some(hz),
        _ => None,
    }
}

// ── Interval adaptation ───────────────────────────────────────────────────────

/// Adapts recalibration interval according to CPU load.
fn adapt_interval() {
    let load = DRIFT.cpu_load_permil.load(Ordering::Relaxed);
    let new_interval = if load < IDLE_THRESHOLD_PERMIL {
        RECAL_INTERVAL_IDLE_NS
    } else if load > HEAVY_THRESHOLD_PERMIL {
        RECAL_INTERVAL_HEAVY_NS
    } else {
        RECAL_INTERVAL_NORMAL_NS
    };
    DRIFT.interval_ns.store(new_interval, Ordering::Relaxed);
}

/// Updates current CPU load (called from tick handler).
/// `load_permil`: load in permille (0–1000, e.g. 500 = 50%).
pub fn update_cpu_load(load_permil: u32) {
    DRIFT
        .cpu_load_permil
        .store(load_permil.min(1000), Ordering::Relaxed);
}

// ── Internal validations ──────────────────────────────────────────────────────

/// Checks that a measured frequency is within reasonable range [500 MHz, 10 GHz].
#[inline(always)]
fn is_plausible_hz(hz: u64) -> bool {
    (500_000_000..=10_000_000_000).contains(&hz)
}

// ── Local assembly primitive ──────────────────────────────────────────────────

/// Reads TSC directly (non-serialized RDTSC, used for drift measurements).
/// RULE DRIFT-CIRCULAR-01: Never ktime_get_ns() in this context.
#[inline(always)]
fn rdtsc_direct() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
    }
    ((hi as u64) << 32) | lo as u64
}

// ── Diagnostics ───────────────────────────────────────────────────────────────

/// Total number of successful recalibrations since boot.
pub fn drift_recal_count() -> u64 {
    DRIFT.recal_count.load(Ordering::Relaxed)
}

/// Number of failed recalibration attempts.
pub fn drift_fail_count() -> u64 {
    DRIFT.fail_count.load(Ordering::Relaxed)
}

/// Number of monotonicity corrections applied.
pub fn drift_monotone_fixes() -> u64 {
    DRIFT.monotone_fixes.load(Ordering::Relaxed)
}

/// Last measured frequency before PLL.
pub fn drift_last_measured_hz() -> u64 {
    DRIFT.last_measured_hz.load(Ordering::Relaxed)
}

/// Last applied frequency after PLL.
pub fn drift_last_applied_hz() -> u64 {
    DRIFT.last_applied_hz.load(Ordering::Relaxed)
}

/// Complete snapshot of drift state for profiling tools.
pub struct DriftSnapshot {
    pub recal_count: u64,
    pub fail_count: u64,
    pub monotone_fixes: u64,
    pub measured_hz: u64,
    pub applied_hz: u64,
    pub interval_ns: u64,
    pub pll_locked: bool,
}

pub fn drift_snapshot() -> DriftSnapshot {
    DriftSnapshot {
        recal_count: DRIFT.recal_count.load(Ordering::Relaxed),
        fail_count: DRIFT.fail_count.load(Ordering::Relaxed),
        monotone_fixes: DRIFT.monotone_fixes.load(Ordering::Relaxed),
        measured_hz: DRIFT.last_measured_hz.load(Ordering::Relaxed),
        applied_hz: DRIFT.last_applied_hz.load(Ordering::Relaxed),
        interval_ns: DRIFT.interval_ns.load(Ordering::Relaxed),
        pll_locked: pll::pll_locked(),
    }
}
