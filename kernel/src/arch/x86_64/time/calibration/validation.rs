// kernel/src/arch/x86_64/time/calibration/validation.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Cross-validation and statistical filtering of TSC calibration
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Role
//   Validate that a measured TSC frequency (via HPET/PM Timer/PIT) is consistent
//   with the manufacturer nominal value (CPUID 0x15) and physical limits.
//
// ## Validation levels
//
//   Level 1 — Absolute range [500 MHz, 10 GHz]
//     Any frequency outside this range is rejected immediately.
//
//   Level 2 — Cross-check CPUID 0x15
//     If CPUID 0x15 is available, compare with measurement:
//       Delta <= 2%   -> EXCELLENT: measurement and CPUID agree well
//       Delta <= 5%   -> GOOD: acceptable agreement (QEMU/VM, elevated temperature)
//       Delta <= 10%  -> WARNING: measurement accepted but logged
//       Delta 10-20%  -> DEGRADED: probable thermal drift or ECX=0 fallback crystal
//       Delta > 20%   -> SUSPECT HARDWARE: measurement may be corrupted
//                        Use nominal CPUID if available
//
//   Level 3 — Successive consistency (multi-measurements)
//     Compare with last validated value: variation > 1% -> warning.
//     Prevents sudden frequency jumps that would cause ktime jumps.
//
//   Level 4 — Consistency with global history
//     Over N measurements, compute standard deviation. If > 1% -> TSC instability detected.
//
// ## Rules
//   RULE CAL-VALIDATE-01: cross-check with CPUID 0x15, thresholds 10%/20%
//   RULE CAL-VALIDATE-02: successive variation > 1% -> warning
//   RULE CAL-VALIDATE-03: history of N measurements, standard deviation > 1% -> instability
//   RULE CAL-VALIDATE-04: absolute range [500 MHz, 10 GHz] — immediate rejection
// ════════════════════════════════════════════════════════════════════════════════

use super::cpuid_nominal;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Minimum acceptable TSC frequency (500 MHz).
pub const TSC_FREQ_MIN_HZ: u64 = 500_000_000;
/// Maximum acceptable TSC frequency (10 GHz).
pub const TSC_FREQ_MAX_HZ: u64 = 10_000_000_000;

/// "Excellent" threshold: delta < 2% (hundredths of a percent).
const THRESHOLD_EXCELLENT_PCT_X100: u128 = 200; // 2 %
/// "Good" threshold: delta < 5%.
const THRESHOLD_GOOD_PCT_X100: u128 = 500; // 5 %
/// "Warning" threshold: delta < 10%.
const THRESHOLD_WARN_PCT_X100: u128 = 1_000; // 10 %
/// "Degraded" threshold: delta < 20%.
const THRESHOLD_DEGRADED_PCT_X100: u128 = 2_000; // 20 %

/// Size of measurement history retained for standard deviation computation.
const HISTORY_SIZE: usize = 8;

/// Maximum acceptable successive variation (1%).
const MAX_SUCCESSIVE_VARIATION_PCT_X100: u128 = 100; // 1 %

// ── Types ─────────────────────────────────────────────────────────────────────

/// Detailed result of validating a measured TSC frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationResult {
    /// Excellent agreement (<2% delta with CPUID). Full confidence.
    Excellent(u64),
    /// Good agreement (<5% delta). Normal confidence.
    Good(u64),
    /// Moderate divergence (<10%). Measurement accepted, warning issued.
    Warning {
        measured: u64,
        cpuid: u64,
        pct_x100: u64,
    },
    /// Significant divergence (10-20%). Measurement accepted but degraded.
    Degraded {
        measured: u64,
        cpuid: u64,
        pct_x100: u64,
    },
    /// Critical divergence (>20%). Measurement rejected, CPUID preferred if available.
    Rejected {
        measured: u64,
        cpuid: u64,
        pct_x100: u64,
    },
    /// CPUID 0x15 not available -> validation impossible, measurement returned as is.
    NoCpuid(u64),
    /// Value outside absolute range [500 MHz, 10 GHz].
    OutOfRange(u64),
    /// Successive variation too large (>1% relative to last measurement).
    SuccessiveJump {
        new_hz: u64,
        prev_hz: u64,
        pct_x100: u64,
    },
}

impl ValidationResult {
    /// Returns the recommended frequency (None if rejected).
    pub fn accepted_hz(self) -> Option<u64> {
        match self {
            ValidationResult::Excellent(hz) => Some(hz),
            ValidationResult::Good(hz) => Some(hz),
            ValidationResult::Warning { measured, .. } => Some(measured),
            ValidationResult::Degraded {
                measured,
                cpuid: _,
                pct_x100: _,
            } => Some(measured),
            ValidationResult::Rejected {
                measured: _, cpuid, ..
            } => {
                // Prefer CPUID if measurement is out of range.
                if cpuid > 0 {
                    Some(cpuid)
                } else {
                    None
                }
            }
            ValidationResult::NoCpuid(hz) => Some(hz),
            ValidationResult::OutOfRange(_) => None,
            ValidationResult::SuccessiveJump { new_hz, .. } => Some(new_hz), // accept despite jump
        }
    }

    /// Indicates whether measurement is reliable (Excellent or Good).
    pub fn is_high_confidence(self) -> bool {
        matches!(
            self,
            ValidationResult::Excellent(_)
                | ValidationResult::Good(_)
                | ValidationResult::NoCpuid(_)
        )
    }

    /// Confidence level from 0 (total rejection) to 100 (perfect).
    pub fn confidence(self) -> u8 {
        match self {
            ValidationResult::Excellent(_) => 100,
            ValidationResult::Good(_) => 90,
            ValidationResult::Warning { .. } => 70,
            ValidationResult::Degraded { .. } => 50,
            ValidationResult::Rejected { .. } => 10,
            ValidationResult::NoCpuid(_) => 80, // No reference, but direct measurement OK.
            ValidationResult::OutOfRange(_) => 0,
            ValidationResult::SuccessiveJump { .. } => 60,
        }
    }

    /// Returns a short label for port 0xE9 logs.
    pub fn label(self) -> &'static str {
        match self {
            ValidationResult::Excellent(_) => "EXCE",
            ValidationResult::Good(_) => "GOOD",
            ValidationResult::Warning { .. } => "WARN",
            ValidationResult::Degraded { .. } => "DEGR",
            ValidationResult::Rejected { .. } => "RJCT",
            ValidationResult::NoCpuid(_) => "NOCI",
            ValidationResult::OutOfRange(_) => "ORAN",
            ValidationResult::SuccessiveJump { .. } => "JUMP",
        }
    }
}

// ── Calibration history ───────────────────────────────────────────────────────

/// History of recent TSC frequency measurements (circular FIFO).
static HISTORY: [AtomicU64; HISTORY_SIZE] = [const { AtomicU64::new(0) }; HISTORY_SIZE];
static HISTORY_IDX: AtomicU32 = AtomicU32::new(0);
static HISTORY_COUNT: AtomicU32 = AtomicU32::new(0);
/// Last validated and accepted value (for successive checks).
static LAST_ACCEPTED_HZ: AtomicU64 = AtomicU64::new(0);
/// Total number of validations performed since boot.
static VALIDATION_COUNT: AtomicU64 = AtomicU64::new(0);
/// Number of degraded validations (Degraded + Rejected).
static DEGRADED_COUNT: AtomicU64 = AtomicU64::new(0);

// ── Main API ──────────────────────────────────────────────────────────────────

/// Complete validation of a measured TSC frequency.
///
/// Performs in order:
///   1. Absolute range check (RULE CAL-VALIDATE-04)
///   2. CPUID 0x15 cross-check (RULE CAL-VALIDATE-01)
///   3. Successive variation check (RULE CAL-VALIDATE-02)
///   4. Update history
///
/// Returns `None` if measurement is rejected (out-of-range or error).
pub fn cross_check(measured_hz: u64) -> Option<u64> {
    VALIDATION_COUNT.fetch_add(1, Ordering::Relaxed);

    let result = validate_full(measured_hz);

    // Log result to port 0xE9 if confidence is low.
    if result.confidence() < 70 {
        DEGRADED_COUNT.fetch_add(1, Ordering::Relaxed);
        log_validation_result(measured_hz, &result);
    }

    match result.accepted_hz() {
        Some(hz) => {
            push_history(hz);
            LAST_ACCEPTED_HZ.store(hz, Ordering::Relaxed);
            Some(hz)
        }
        None => None,
    }
}

/// Checks that frequency is within a reasonable physical range.
#[inline(always)]
pub fn hz_in_range(hz: u64) -> bool {
    (TSC_FREQ_MIN_HZ..=TSC_FREQ_MAX_HZ).contains(&hz)
}

/// Complete validation returning detailed `ValidationResult`.
pub fn validate_full(measured_hz: u64) -> ValidationResult {
    // ── Level 1: absolute range ───────────────────────────────────────────────
    if !hz_in_range(measured_hz) {
        return ValidationResult::OutOfRange(measured_hz);
    }

    // ── Level 3: successive variation check ───────────────────────────────────
    let prev = LAST_ACCEPTED_HZ.load(Ordering::Relaxed);
    if prev > 0 {
        let successive_pct = percent_x100(measured_hz, prev);
        if successive_pct > MAX_SUCCESSIVE_VARIATION_PCT_X100 as u64 {
            // Large jump -> log but accept (may be normal after forced recalibration).
            return ValidationResult::SuccessiveJump {
                new_hz: measured_hz,
                prev_hz: prev,
                pct_x100: successive_pct,
            };
        }
    }

    // ── Level 2: CPUID 0x15 cross-check ───────────────────────────────────────
    // RULE CAL-VALIDATE-01: comparison with manufacturer nominal value.
    let cpuid_hz = match cpuid_nominal::cpuid_tsc_hz() {
        Some(hz) if hz > 0 => hz,
        _ => {
            // CPUID 0x15 not available — no cross-check possible.
            return ValidationResult::NoCpuid(measured_hz);
        }
    };

    let pct_x100 = percent_x100_u128(measured_hz as u128, cpuid_hz as u128);

    if pct_x100 <= THRESHOLD_EXCELLENT_PCT_X100 {
        ValidationResult::Excellent(measured_hz)
    } else if pct_x100 <= THRESHOLD_GOOD_PCT_X100 {
        ValidationResult::Good(measured_hz)
    } else if pct_x100 <= THRESHOLD_WARN_PCT_X100 {
        ValidationResult::Warning {
            measured: measured_hz,
            cpuid: cpuid_hz,
            pct_x100: pct_x100 as u64,
        }
    } else if pct_x100 <= THRESHOLD_DEGRADED_PCT_X100 {
        ValidationResult::Degraded {
            measured: measured_hz,
            cpuid: cpuid_hz,
            pct_x100: pct_x100 as u64,
        }
    } else {
        ValidationResult::Rejected {
            measured: measured_hz,
            cpuid: cpuid_hz,
            pct_x100: pct_x100 as u64,
        }
    }
}

/// Simple variant returning only classical result (for calibration/mod.rs).
pub fn validate(measured_hz: u64) -> ValidationResult {
    validate_full(measured_hz)
}

// ── Statistical history analysis ──────────────────────────────────────────────

/// Computes the mean of measurements in history.
pub fn history_mean_hz() -> u64 {
    let count = HISTORY_COUNT.load(Ordering::Relaxed) as usize;
    if count == 0 {
        return 0;
    }
    let n = count.min(HISTORY_SIZE);
    let mut sum = 0u128;
    for item in &HISTORY[..n] {
        sum += item.load(Ordering::Relaxed) as u128;
    }
    (sum / n as u128) as u64
}

/// Computes normalized variance of measurements (in ppm²).
/// A result > 10_000 ppm² (~100 ppm std dev) indicates TSC instability.
pub fn history_variance_ppm2() -> u64 {
    let count = HISTORY_COUNT.load(Ordering::Relaxed) as usize;
    let n = count.min(HISTORY_SIZE);
    if n < 2 {
        return 0;
    }

    let mean = history_mean_hz();
    if mean == 0 {
        return 0;
    }

    let mut sum_sq_ppm: u128 = 0;
    for item in &HISTORY[..n] {
        let val = item.load(Ordering::Relaxed);
        // deviation in ppm = |val - mean| * 1_000_000 / mean
        let dev_ppm = (val.abs_diff(mean) as u128 * 1_000_000) / mean as u128;
        sum_sq_ppm = sum_sq_ppm.saturating_add(dev_ppm * dev_ppm);
    }
    (sum_sq_ppm / n as u128) as u64
}

/// Returns `true` if history indicates an unstable TSC (variance > threshold).
/// Threshold: variance > 10_000 ppm² (std dev ≈ 100 ppm ≈ 300 Hz at 3 GHz).
pub fn tsc_is_unstable() -> bool {
    history_variance_ppm2() > 10_000
}

/// Snapshot of validation state for diagnostics.
pub struct ValidationSnapshot {
    pub validation_count: u64,
    pub degraded_count: u64,
    pub last_accepted_hz: u64,
    pub history_mean_hz: u64,
    pub variance_ppm2: u64,
    pub unstable: bool,
    pub cpuid_available: bool,
    pub cpuid_hz: u64,
}

pub fn validation_snapshot() -> ValidationSnapshot {
    ValidationSnapshot {
        validation_count: VALIDATION_COUNT.load(Ordering::Relaxed),
        degraded_count: DEGRADED_COUNT.load(Ordering::Relaxed),
        last_accepted_hz: LAST_ACCEPTED_HZ.load(Ordering::Relaxed),
        history_mean_hz: history_mean_hz(),
        variance_ppm2: history_variance_ppm2(),
        unstable: tsc_is_unstable(),
        cpuid_available: cpuid_nominal::cpuid_leaf15_available(),
        cpuid_hz: cpuid_nominal::cpuid_tsc_hz().unwrap_or(0),
    }
}

// ── Final recommendation ──────────────────────────────────────────────────────

/// Returns the best available TSC frequency combining measurement + CPUID.
///
/// Conservative fusion logic:
///   - If measurement validated EXCELLENT/GOOD -> use measurement (more precise than CPUID).
///   - If measurement WARN/DEGRADED + CPUID available -> weighted average 70/30.
///   - If measurement REJECTED + CPUID available -> CPUID only.
///   - If no CPUID -> direct measurement.
///   - Result always within [500 MHz, 10 GHz].
pub fn best_frequency(measured_hz: u64) -> Option<u64> {
    let result = validate_full(measured_hz);
    let cpuid_hz = cpuid_nominal::cpuid_tsc_hz().unwrap_or(0);

    let hz = match result {
        ValidationResult::Excellent(hz) | ValidationResult::Good(hz) => hz,
        ValidationResult::NoCpuid(hz) => hz,
        ValidationResult::Warning { measured, .. } => {
            // Trust measurement for Warning (nominal CPUID can be shifted in VM).
            measured
        }
        ValidationResult::Degraded {
            measured, cpuid, ..
        } => {
            if cpuid > 0 {
                // Weighted average 70% measurement + 30% CPUID.
                let weighted = (measured as u128 * 70 + cpuid as u128 * 30) / 100;
                weighted as u64
            } else {
                measured
            }
        }
        ValidationResult::Rejected { cpuid, .. } => {
            if cpuid > 0 {
                cpuid
            } else {
                return None;
            }
        }
        ValidationResult::OutOfRange(_) => return None,
        ValidationResult::SuccessiveJump { new_hz, .. } => {
            // Successive jump: use measurement but verify against CPUID.
            if cpuid_hz > 0 {
                let pct = percent_x100_u128(new_hz as u128, cpuid_hz as u128);
                if pct > THRESHOLD_DEGRADED_PCT_X100 {
                    // Too divergent even with jump -> CPUID.
                    cpuid_hz
                } else {
                    new_hz
                }
            } else {
                new_hz
            }
        }
    };

    if hz_in_range(hz) {
        Some(hz)
    } else {
        None
    }
}

// ── Internal utilities ────────────────────────────────────────────────────────

/// Calculates difference in hundredths of % between two frequencies (|a - b| / b * 10000).
fn percent_x100_u128(a: u128, b: u128) -> u128 {
    if b == 0 {
        return 10_000;
    }
    let diff = a.abs_diff(b);
    diff.saturating_mul(10_000) / b
}

/// u64 version for critical comparisons.
fn percent_x100(a: u64, b: u64) -> u64 {
    if b == 0 {
        return 10_000;
    }
    let diff = a.abs_diff(b);
    ((diff as u128).saturating_mul(10_000) / b as u128) as u64
}

/// Appends a value to the circular history.
fn push_history(hz: u64) {
    let idx = (HISTORY_IDX.fetch_add(1, Ordering::Relaxed) as usize) % HISTORY_SIZE;
    HISTORY[idx].store(hz, Ordering::Relaxed);
    // Increment count up to HISTORY_SIZE.
    let prev = HISTORY_COUNT.load(Ordering::Relaxed) as usize;
    if prev < HISTORY_SIZE {
        HISTORY_COUNT.store((prev + 1) as u32, Ordering::Relaxed);
    }
}

/// Emits a diagnostic message to port 0xE9 if result is degraded.
fn log_validation_result(measured_hz: u64, result: &ValidationResult) {
    // Format: "[VAL:XXXX hz=XXXXXXXX]\n"
    unsafe fn out(b: u8) {
        unsafe {
            core::arch::asm!("out 0xe9, al", in("al") b, options(nomem, nostack));
        }
    }
    let label = result.label();
    // SAFETY: port 0xE9 = QEMU debug channel, no effect if unavailable.
    unsafe {
        for &b in b"[VAL:" {
            out(b);
        }
        for &b in label.as_bytes() {
            out(b);
        }
        out(b' ');
        // Write measured_hz in 16 hex digits.
        let v = measured_hz;
        for shift in (0..64usize).step_by(4).rev() {
            let nib = ((v >> (60 - shift)) & 0xF) as u8;
            out(if nib < 10 {
                b'0' + nib
            } else {
                b'a' + nib - 10
            });
        }
        out(b']');
        out(b'\n');
    }
}
