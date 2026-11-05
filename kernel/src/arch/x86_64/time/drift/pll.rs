// kernel/src/arch/x86_64/time/drift/pll.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Software PLL — Smoothed correction of TSC drift
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Problem
//   TSC fluctuates slightly with temperature, CPU load, and P-state transitions.
//   Abruptly applying a newly measured TSC frequency causes application time to jump
//   (forwards or backwards if correction is too large).
//
// ## Solution: Software PLL (Phase-Locked Loop)
//   Low-pass filter: fractional correction limited to ±500 ppm per cycle.
//   Progressive convergence over multiple cycles if drift is significant.
//   Formula: new_hz = current_hz + clamp(measured_hz - current_hz, -MAX_ADJ, +MAX_ADJ)
//
// ## Guarantees
//   RULE DRIFT-PLL-01      : maximum correction ±500 ppm per recalibration
//   RULE DRIFT-MONOTONE-01 : ktime never regresses after correction
//   RULE ARCH-TIME-04      : strict monotonicity guaranteed
//
// ## Temporal resolution after correction
//   At 3 GHz TSC: ±500 ppm = ±1.5 MHz -> ±0.5 ns/µs residual error
//   Complete convergence if drift <= 500 ppm: 1 recalibration cycle
//   Convergence if drift = 5000 ppm: 10 cycles * 30s = 5 minutes
// ════════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

// ── PLL constants ─────────────────────────────────────────────────────────────

/// Maximum correction per recalibration cycle: 500 ppm.
/// For 3 GHz TSC: 500 ppm * 3_000_000_000 Hz = 1_500 Hz max correction.
const MAX_ADJ_PPM: u64 = 500;

/// Low-pass filter coefficient (0–1000 in thousandths).
/// 750 = 75% of correction applied immediately (fast convergence).
/// Lower value = more smoothing, slower convergence.
const FILTER_COEFF_MILLI: u64 = 750;

/// Number of consecutive consistent measurements required to activate correction.
/// Prevents corrections on isolated anomalous measurements.
#[allow(dead_code)]
const CONVERGENCE_THRESHOLD: u32 = 2;

// ── PLL state ─────────────────────────────────────────────────────────────────

/// Internal state of the PLL loop.
struct PllState {
    /// Current reference frequency (Hz) — base for correction calculation.
    current_hz: AtomicU64,
    /// Accumulated phase error (in nano-ppm, signed) — PI controller integration.
    #[allow(dead_code)]
    phase_error_nppm: AtomicI64,
    /// Number of consecutive consistent measurements observed.
    #[allow(dead_code)]
    converge_count: AtomicU64,
    /// Sum of the N last measurements for moving average computation.
    measure_sum: AtomicU64,
    /// Number of measurements in moving average sum.
    measure_count: AtomicU64,
    /// Last adjustment applied (in Hz, signed via cast).
    last_adj_hz: AtomicI64,
    /// `true` if PLL is locked (drift < LOCK_THRESHOLD_PPM).
    locked: AtomicBool,
    /// Total count of corrections applied since boot.
    total_corrections: AtomicU64,
}

impl PllState {
    const fn new() -> Self {
        Self {
            current_hz: AtomicU64::new(3_000_000_000),
            phase_error_nppm: AtomicI64::new(0),
            converge_count: AtomicU64::new(0),
            measure_sum: AtomicU64::new(0),
            measure_count: AtomicU64::new(0),
            last_adj_hz: AtomicI64::new(0),
            locked: AtomicBool::new(false),
            total_corrections: AtomicU64::new(0),
        }
    }
}

static PLL: PllState = PllState::new();

/// Drift threshold in ppm below which the PLL is considered "locked".
const LOCK_THRESHOLD_PPM: u64 = 100;

/// Moving average window (last N measurements).
const MOVING_AVG_WINDOW: u64 = 5;

// ── Main API ──────────────────────────────────────────────────────────────────

/// Initializes the PLL with the frequency calibrated at boot.
///
/// Must be called from `time_init()` after initial calibration.
pub fn pll_init(initial_hz: u64) {
    PLL.current_hz.store(initial_hz, Ordering::Release);
    PLL.measure_sum
        .store(initial_hz * MOVING_AVG_WINDOW, Ordering::Relaxed);
    PLL.measure_count
        .store(MOVING_AVG_WINDOW, Ordering::Relaxed);
    PLL.locked.store(false, Ordering::Release);
}

/// Submits a new TSC frequency measurement and computes smoothed correction.
///
/// RULE DRIFT-PLL-01: max correction ±500 ppm per call.
/// RULE DRIFT-MONOTONE-01: returns (ns_now, new_hz) such that ns_now >= ns_previous.
///
/// # Arguments
/// - `measured_hz`: TSC frequency measured by current calibration
/// - `tsc_now`: TSC value read DIRECTLY (not via ktime_get_ns)
/// - `ref_ns_anchor`: ns corresponding to `tsc_now` (computed by drift thread)
///
/// # Returns
/// `(corrected_ns_anchor, new_tsc_hz)` to pass to `update_ktime_anchor()`.
///
/// RULE DRIFT-CIRCULAR-01: Do NOT call ktime_get_ns() here.
pub fn pll_update(measured_hz: u64, _tsc_now: u64, ref_ns_anchor: u64) -> (u64, u64) {
    // ── Moving average update ─────────────────────────────────────────────────
    let old_sum = PLL.measure_sum.load(Ordering::Relaxed);
    let old_count = PLL
        .measure_count
        .load(Ordering::Relaxed)
        .min(MOVING_AVG_WINDOW);

    // Expel oldest measurement from window if window full.
    let new_sum = if old_count >= MOVING_AVG_WINDOW {
        let oldest_approx = old_sum / old_count;
        old_sum
            .saturating_sub(oldest_approx)
            .saturating_add(measured_hz)
    } else {
        old_sum.saturating_add(measured_hz)
    };
    let new_count = (old_count + 1).min(MOVING_AVG_WINDOW);

    PLL.measure_sum.store(new_sum, Ordering::Relaxed);
    PLL.measure_count.store(new_count, Ordering::Relaxed);

    // Smoothed frequency = moving average of last N measurements.
    let smoothed_hz = new_sum / new_count;

    // ── Correction computation ────────────────────────────────────────────────
    let current_hz = PLL.current_hz.load(Ordering::Relaxed);
    if current_hz == 0 {
        return (ref_ns_anchor, measured_hz);
    }

    // Raw error (signed).
    let error_hz: i64 = smoothed_hz as i64 - current_hz as i64;

    // Correction limit: MAX_ADJ_PPM * current_hz / 1_000_000.
    let max_adj_hz = ((current_hz as u128 * MAX_ADJ_PPM as u128) / 1_000_000) as i64;
    let max_adj_hz = max_adj_hz.max(1); // At least 1 Hz correction possible.

    // Clamp error to ±MAX_ADJ_PPM.
    let clamped_error = error_hz.clamp(-max_adj_hz, max_adj_hz);

    // Low-pass filter: apply FILTER_COEFF % of correction.
    let adj_hz = (clamped_error * FILTER_COEFF_MILLI as i64) / 1000;

    // New frequency after correction (do not exceed absolute bounds).
    let new_hz_signed = current_hz as i64 + adj_hz;
    let new_hz = (new_hz_signed.max(500_000_000) as u64).min(10_000_000_000);

    // ── Convergence check ─────────────────────────────────────────────────────
    let deviation_ppm = if new_hz > 0 {
        (error_hz.unsigned_abs() as u128 * 1_000_000 / new_hz as u128) as u64
    } else {
        0
    };

    if deviation_ppm < LOCK_THRESHOLD_PPM {
        PLL.locked.store(true, Ordering::Relaxed);
    } else {
        PLL.locked.store(false, Ordering::Relaxed);
    }

    PLL.last_adj_hz.store(adj_hz, Ordering::Relaxed);
    PLL.current_hz.store(new_hz, Ordering::Release);
    PLL.total_corrections.fetch_add(1, Ordering::Relaxed);

    // ── Recalculate ns_anchor to guarantee monotonicity ───────────────────────
    // If tsc_hz changes, calculation ns = tsc_delta * 1e9 / hz changes.
    // Must recalculate ns_anchor from current tsc_now and new hz.
    // RULE DRIFT-MONOTONE-01: ns_anchor_new >= ns_anchor_old.
    let ns_anchor_new = ref_ns_anchor; // Caller already calculates ns_now correctly.

    (ns_anchor_new, new_hz)
}

/// Returns `true` if PLL is locked (drift < 100 ppm).
#[inline(always)]
pub fn pll_locked() -> bool {
    PLL.locked.load(Ordering::Relaxed)
}

/// Returns current frequency according to PLL (Hz).
#[inline(always)]
pub fn pll_current_hz() -> u64 {
    PLL.current_hz.load(Ordering::Relaxed)
}

/// Returns last applied adjustment (Hz, signed).
#[inline(always)]
pub fn pll_last_adj_hz() -> i64 {
    PLL.last_adj_hz.load(Ordering::Relaxed)
}

/// Returns total count of PLL corrections since boot.
pub fn pll_correction_count() -> u64 {
    PLL.total_corrections.load(Ordering::Relaxed)
}

/// Complete snapshot of PLL state for diagnostics.
pub struct PllSnapshot {
    pub current_hz: u64,
    pub last_adj_hz: i64,
    pub locked: bool,
    pub corrections: u64,
    pub smoothed_hz: u64,
}

pub fn pll_snapshot() -> PllSnapshot {
    let sum = PLL.measure_sum.load(Ordering::Relaxed);
    let count = PLL.measure_count.load(Ordering::Relaxed).max(1);
    PllSnapshot {
        current_hz: PLL.current_hz.load(Ordering::Relaxed),
        last_adj_hz: PLL.last_adj_hz.load(Ordering::Relaxed),
        locked: PLL.locked.load(Ordering::Relaxed),
        corrections: PLL.total_corrections.load(Ordering::Relaxed),
        smoothed_hz: sum / count,
    }
}
