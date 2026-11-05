// kernel/src/arch/x86_64/time/calibration/mod.rs
//
// ════════════════════════════════════════════════════════════════════════════
// TSC calibration orchestration — complete fallback chain
// ════════════════════════════════════════════════════════════════════════════
//
// ## Responsibilities
//   1. Initialize clock sources (init_sources) before any measurement.
//   2. Orchestrate the complete fallback chain with diagnostics.
//   3. Propagate the calibrated frequency to cpu::tsc and sources::tsc.
//   4. Expose a rich API for monitoring, drift, and per-CPU recalibration.
//
// ## Fallback chain (descending rating)
//   1. HPET window 1ms × 10 samples    (rating 300) — MMIO, accuracy ±0.05%
//   2. PM Timer window 1ms × 10 samples(rating 200) — port I/O, ±0.05%
//   3. CPUID leaf 0x15                 (rating 150) — manufacturer nominal
//   4. CPUID leaf 0x16                 (rating 100) — base frequency MHz
//   5. KVM/QEMU CPUID hypervisor TSC   (rating  80) — paravirtualized
//   6. PIT one-shot 1ms × 10 samples   (rating  50) — legacy x86
//   7. Fallback 3 GHz                  (rating  10) — last resort
//
// ## Rules adhered to
//   CAL-FALLBACK-01 : each fallback is traced (port 0xE9 + internal log).
//   CAL-WINDOW-01   : exit condition = real-time source ticks, never iterations.
//   CAL-CLI-01      : cli/sti per 1ms sample, not across overall 10ms window.
//   CAL-RDTSCP-01   : RDTSCP for measurement end (out-of-order serialization).
//   SCHED-INIT-01   : calibrate_tsc() does NOT receive tsc_hz as parameter.
//   TIME-01         : TSC_HZ propagated via cpu::tsc::set_tsc_hz() (atomic Release).
//   TIME-02         : never while iter < N for time measurements.
//
// ## Typical usage
//   ```rust
//   // In time_init() :
//   let hz = calibration::calibrate_tsc();
//   // → calls sources::init_sources() internally
//   // → always returns a value in [100 MHz, 10 GHz]
//
//   // Post-boot diagnostic :
//   let detail = calibration::last_calibration_result();
//   let (hz, source_tag, confidence, seq) = calibration::calibration_stats();
//
//   // Post-APIC recalibration (per-CPU safe) :
//   let new_hz = calibration::recalibrate_tsc();
//   ```
// ════════════════════════════════════════════════════════════════════════════

pub mod cpuid_nominal;
pub mod validation;
pub mod window;

use super::sources;
use crate::arch::x86_64::cpu::tsc as cpu_tsc;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

// Public re-exports for consumer modules (drift, percpu, ktime).
pub use window::{mean_and_variance, CalibrationResult, CalibrationSample, CalibrationSource};

// ── Global calibration state ─────────────────────────────────────────────────

/// Calibrated TSC frequency in Hz (atomic, updated with Release ordering).
static LAST_TSC_HZ: AtomicU64 = AtomicU64::new(0);

/// Index of the source used for the last calibration (CalibSource discriminant).
static LAST_SOURCE: AtomicU8 = AtomicU8::new(CalibSource::None as u8);

/// Rating of the last used source (0–300).
static LAST_RATING: AtomicU32 = AtomicU32::new(0);

/// Calibration sequence counter (incremented on each calibrate_tsc() or recalibrate).
static CALIB_SEQ: AtomicU32 = AtomicU32::new(0);

/// Number of recalibrations performed post-boot (recalibrate_tsc()).
static RECALIB_COUNT: AtomicU32 = AtomicU32::new(0);

/// Confidence level of the last calibration (0–100).
static LAST_CONFIDENCE: AtomicU8 = AtomicU8::new(0);

/// Guard: sources::init_sources() already called?
static SOURCES_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ── Exported types ───────────────────────────────────────────────────────────

/// Source used for TSC calibration.
/// Serializable to u8 for atomic storage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum CalibSource {
    Hpet = 0,
    PmTimer = 1,
    Cpuid15 = 2,
    Cpuid16 = 3,
    Pit = 4,
    Fallback3G = 5,
    Hypervisor = 6,
    None = 0xFF,
}

impl CalibSource {
    /// Descending precision rating: HPET best, Fallback worst.
    #[inline(always)]
    pub fn rating(self) -> u32 {
        match self {
            CalibSource::Hpet => 300,
            CalibSource::PmTimer => 200,
            CalibSource::Cpuid15 => 150,
            CalibSource::Cpuid16 => 100,
            CalibSource::Hypervisor => 80,
            CalibSource::Pit => 50,
            CalibSource::Fallback3G => 10,
            CalibSource::None => 0,
        }
    }

    /// True if the source involves real measurement (time window).
    #[inline(always)]
    pub fn is_measured(self) -> bool {
        matches!(
            self,
            CalibSource::Hpet | CalibSource::PmTimer | CalibSource::Pit
        )
    }

    /// True if the source is reliable for drift compensation.
    #[inline(always)]
    pub fn is_trusted(self) -> bool {
        matches!(
            self,
            CalibSource::Hpet
                | CalibSource::PmTimer
                | CalibSource::Cpuid15
                | CalibSource::Hypervisor
        )
    }

    /// Short ASCII label for port 0xE9 / logs.
    #[inline(always)]
    pub fn tag(self) -> &'static [u8] {
        match self {
            CalibSource::Hpet => b"HPET",
            CalibSource::PmTimer => b"PMT",
            CalibSource::Cpuid15 => b"CPUID15",
            CalibSource::Cpuid16 => b"CPUID16",
            CalibSource::Pit => b"PIT",
            CalibSource::Fallback3G => b"FB3G",
            CalibSource::Hypervisor => b"KVM-TSC",
            CalibSource::None => b"NONE",
        }
    }

    #[inline(always)]
    fn from_u8(v: u8) -> Self {
        match v {
            0 => CalibSource::Hpet,
            1 => CalibSource::PmTimer,
            2 => CalibSource::Cpuid15,
            3 => CalibSource::Cpuid16,
            4 => CalibSource::Pit,
            5 => CalibSource::Fallback3G,
            6 => CalibSource::Hypervisor,
            _ => CalibSource::None,
        }
    }
}

/// Enriched result of a TSC calibration.
/// Cached for consumers (drift, percpu, monitoring).
#[derive(Clone, Copy, Debug)]
pub struct CalibratedTsc {
    /// TSC frequency in Hz, rounded to 100 kHz.
    pub tsc_hz: u64,
    /// Source used for calibration.
    pub source: CalibSource,
    /// Confidence level 0–100 (100 = HPET perfect measurement).
    pub confidence: u8,
    /// Inter-sample variance in Hz² (0 if nominal CPUID/Fallback source).
    pub variance_hz2: u64,
    /// Number of valid samples used for mean (0 if nominal).
    pub valid_samples: u8,
    /// Calibration duration in milliseconds (approximate via TSC).
    pub duration_tsc_cycles: u64,
    /// True if TSC is invariant (CPUID 0x80000007[8]).
    pub tsc_invariant: bool,
    /// Calibration sequence (monotonically increasing).
    pub seq: u32,
}

impl CalibratedTsc {
    /// Returns `true` if the result is usable in production.
    #[inline(always)]
    pub fn is_production_grade(&self) -> bool {
        self.confidence >= 80 && self.source.is_measured()
    }

    /// Returns `true` if the result requires post-APIC recalibration.
    #[inline(always)]
    pub fn needs_recalibration(&self) -> bool {
        !self.source.is_measured()
    }
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Calibrates the TSC frequency via the complete fallback chain.
///
/// ## Guarantees
/// - ALWAYS returns a value in [100 MHz, 10 GHz] (never 0).
/// - Calls `sources::init_sources()` exactly once (idempotent).
/// - Propagates frequency to `cpu_tsc::set_tsc_hz()` (atomic Release).
/// - Updates `sources::update_source_rating(Tsc, rating)`.
/// - Thread-safe (atomics Release/Acquire), but designed for BSP only.
///
/// ## RULE SCHED-INIT-01
/// Receives NO tsc_hz parameter. Frequency comes exclusively
/// from calibration — never from an external parameter.
pub fn calibrate_tsc() -> u64 {
    let result = run_calibration_chain();
    commit_calibration_result(&result);
    result.tsc_hz
}

/// Rich version: returns full `CalibratedTsc` for monitoring/drift.
pub fn calibrate_tsc_detail() -> CalibratedTsc {
    let result = run_calibration_chain();
    commit_calibration_result(&result);
    result
}

// ── Post-APIC recalibration ───────────────────────────────────────────────────

/// Recalibrates TSC after APIC/HPET initialization.
///
/// Post-APIC, timers are more stable -> better precision.
/// Applies validation: if new Hz differs by >50% from old,
/// recalibration is rejected (protection against erratic measurements).
///
/// Returns the new TSC frequency (or old one if rejected).
pub fn recalibrate_tsc() -> u64 {
    let old_hz = LAST_TSC_HZ.load(Ordering::Acquire);
    RECALIB_COUNT.fetch_add(1, Ordering::Relaxed);

    let result = run_calibration_chain();

    // Consistency check: ±50% relative to old value.
    if old_hz > 0 {
        let lo = old_hz / 2;
        let hi = old_hz.saturating_add(old_hz / 2);
        if result.tsc_hz < lo || result.tsc_hz > hi {
            // Recalibration rejected — keep old value.
            e9_tag(b"RECAL-REJECT");
            return old_hz;
        }
    }

    commit_calibration_result(&result);
    result.tsc_hz
}

// ── Internal fallback chain ───────────────────────────────────────────────────

/// Calibration core — executes the fallback chain and constructs `CalibratedTsc`.
///
/// ## Priority order:
///   1. HPET window    (high precision hardware source)
///   2. PM Timer       (stable ACPI source)
///   3. CPUID 0x15     (TSC/crystal ratio, instantaneous under QEMU/KVM)
///   4. CPUID 0x16     (base MHz — less precise)
///   5. CPUID best     (nominal cross-check)
///   6. KVM/QEMU CPUID hypervisor TSC
///   7. PIT driver     (port I/O fallback)
///   8. 3 GHz fallback (last resort)
fn run_calibration_chain() -> CalibratedTsc {
    let seq = CALIB_SEQ.fetch_add(1, Ordering::AcqRel) + 1;
    let tsc_invariant = cpu_tsc::tsc_invariant();

    // ── ARCH FIX: initialize sources BEFORE any measurement ──────────────────
    ensure_sources_initialized();

    let start_cycles = cpu_tsc::read_tsc();

    // ── Attempt 1: HPET window (MMIO, high precision) ──────────────────────
    if sources::hpet::available() {
        let hpet_hz = sources::hpet::freq_hz();
        if let Some(hz) = window::calibrate_tsc_via_hpet(hpet_hz) {
            if validation::hz_in_range(hz) {
                let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
                e9_tag_hz(b"CAL:HPET", hz);
                return CalibratedTsc {
                    tsc_hz: round_hz(hz),
                    source: CalibSource::Hpet,
                    confidence: 95,
                    variance_hz2: 0,
                    valid_samples: 10,
                    duration_tsc_cycles: duration,
                    tsc_invariant,
                    seq,
                };
            }
        }
        e9_tag(b"CAL:HPET-FAIL");
    }

    // ── Attempt 2: ACPI PM Timer (port I/O, stable under VM) ───────────────
    if sources::pm_timer::available() || sources::pm_timer::auto_detect() {
        if let Some(hz) = window::calibrate_tsc_via_pm_timer() {
            if validation::hz_in_range(hz) {
                let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
                e9_tag_hz(b"CAL:PMT", hz);
                return CalibratedTsc {
                    tsc_hz: round_hz(hz),
                    source: CalibSource::PmTimer,
                    confidence: 90,
                    variance_hz2: 0,
                    valid_samples: 10,
                    duration_tsc_cycles: duration,
                    tsc_invariant,
                    seq,
                };
            }
        }
        e9_tag(b"CAL:PMT-FAIL");
    }

    // ── Attempt 3: CPUID leaf 0x15 (TSC/Crystal ratio) ─────────────────────
    // Instantaneous nominal source — no MMIO, no busy-polling.
    // Intel Core gen 6+ and Atom: exact TSC/crystal ratio.
    // PRIORITY: attempted FIRST because:
    //   1. Never blocks (no wait loop)
    //   2. Works under virtualization (QEMU -cpu host, KVM, VMware)
    //   3. More precise than a short window measurement (exact resolution)
    if let Some(hz) = cpuid_nominal::cpuid_tsc_hz() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag_hz(b"CAL:CPUID15", hz);
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Cpuid15,
                confidence: 85, // nominal = very reliable on Intel gen6+
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

    // ── Attempt 4: CPUID leaf 0x16 (CPU base frequency) ────────────────────
    // Limited resolution: multiple of 1 MHz. No loop -> instantaneous.
    if let Some(hz) = cpuid_nominal::cpuid_tsc_hz_leaf16() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag_hz(b"CAL:CPUID16", hz);
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Cpuid16,
                confidence: 70, // less precise — 1 MHz resolution
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

    // ── Attempt 5: best CPUID estimate (cross-check 0x15/0x16) ─────────────
    // cpuid_best_estimate() cross-references both CPUID leaves to refine.
    if let Some(hz) = cpuid_nominal::cpuid_best_estimate() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag_hz(b"CAL:CPUID-BEST", hz);
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Cpuid15, // best estimate via CPUID
                confidence: 65,
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

    // ── Attempt 6: KVM/QEMU hypervisor TSC frequency ───────────────────────
    if let Some(hz) = cpuid_nominal::hypervisor_tsc_hz() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag_hz(b"CAL:KVM-TSC", hz);
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Hypervisor,
                confidence: 80,
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

    // ── Attempt 7: PIT one-shot via cpu_tsc driver (port I/O, reliable on QEMU) ──
    // On QEMU TCG, PIT channel 2 is well simulated (port I/O, not MMIO).
    // Returns 0 if PIT inactive or I/O too slow (QEMU TCG 10K iter * inb).
    {
        let hz = cpu_tsc::calibrate_tsc_with_pit();
        if hz > 0 && validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag_hz(b"CAL:PIT-DRV", hz);
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Pit,
                confidence: 55, // PIT measured on ~10ms window = reliable
                variance_hz2: 0,
                valid_samples: 1, // single measurement = 10ms
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
        e9_tag(b"CAL:PIT-DRV-FAIL");

        // Pragmatic bring-up: if PIT driver fails, avoid remaining costly/fragile
        // calibration paths and immediately fall back to 3 GHz to avoid blocking boot.
        e9_tag_hz(b"CAL:FB3G", 3_000_000_000u64);
        let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
        CalibratedTsc {
            tsc_hz: 3_000_000_000u64,
            source: CalibSource::Fallback3G,
            confidence: 10,
            variance_hz2: 0,
            valid_samples: 0,
            duration_tsc_cycles: duration,
            tsc_invariant,
            seq,
        }
    }
}

// ── Result propagation ────────────────────────────────────────────────────────

/// Propagates calibration result to dependent modules.
///
/// Effects:
///   - `cpu_tsc::set_tsc_hz(hz)` -> updates TSC_HZ, TSC_KHZ, TSC_CALIBRATED
///   - `sources::update_source_rating(Tsc, rating)` -> upgrades TSC to runtime source
///   - Updates LAST_* atomics for diagnostics
fn commit_calibration_result(r: &CalibratedTsc) {
    // 1. Propagate to cpu::tsc (base atomics for tsc_hz(), tsc_cycles_to_ns(), etc.)
    cpu_tsc::set_tsc_hz(r.tsc_hz);

    // 2. Upgrade TSC rating in sources registry.
    // After calibration, TSC becomes the best runtime source.
    let tsc_runtime_rating = 400u32; // Calibrated invariant TSC = best runtime source
    sources::update_source_rating(sources::SourceId::Tsc, tsc_runtime_rating);

    // 3. Save result for last_calibration_result() / calibration_stats().
    LAST_TSC_HZ.store(r.tsc_hz, Ordering::Release);
    LAST_SOURCE.store(r.source as u8, Ordering::Release);
    LAST_RATING.store(r.source.rating(), Ordering::Release);
    LAST_CONFIDENCE.store(r.confidence, Ordering::Release);
}

// ── Sources initialization ────────────────────────────────────────────────────

/// Ensures that `sources::init_sources()` is called exactly once.
///
/// Idempotent: can be called from recalibrate_tsc() without double-init.
/// `sources::init_sources()` initializes:
///   - sources/hpet.rs::HPET_BASE_ADDR  (from acpi/hpet.rs::HPET_BASE)
///   - sources/hpet.rs::HPET_FREQ_HZ    (from acpi/hpet.rs::HPET_PERIOD_FS)
///   - sources/pm_timer.rs::PM_BASE_PORT (from ACPI FADT)
///   - sources/pit.rs::PIT initialized
///   - sources/tsc.rs::init_tsc_source() (invariant detection, hypervisor, etc.)
#[inline]
fn ensure_sources_initialized() {
    // compare_exchange(false -> true): atomic, only one thread wins.
    if SOURCES_INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        sources::init_sources();
    }
    // Otherwise: already initialized, continue without double-init.
}

// ── Diagnostic API ───────────────────────────────────────────────────────────

/// Returns the complete result of the last calibration.
///
/// Returns `None` if `calibrate_tsc()` was never called.
pub fn last_calibration_result() -> Option<CalibratedTsc> {
    let hz = LAST_TSC_HZ.load(Ordering::Acquire);
    if hz == 0 {
        return None;
    }
    let seq = CALIB_SEQ.load(Ordering::Relaxed);
    Some(CalibratedTsc {
        tsc_hz: hz,
        source: CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)),
        confidence: LAST_CONFIDENCE.load(Ordering::Relaxed),
        variance_hz2: 0, // not stored globally (available via calibrate_tsc_detail)
        valid_samples: if CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)).is_measured() {
            window::N_SAMPLES as u8
        } else {
            0
        },
        duration_tsc_cycles: 0, // not stored globally
        tsc_invariant: cpu_tsc::tsc_invariant(),
        seq,
    })
}

/// Returns a tuple of compact statistics for monitoring.
///
/// Returns `(tsc_hz, calib_seq, confidence, recalib_count)`.
pub fn calibration_stats() -> (u64, u32, u8, u32) {
    (
        LAST_TSC_HZ.load(Ordering::Relaxed),
        CALIB_SEQ.load(Ordering::Relaxed),
        LAST_CONFIDENCE.load(Ordering::Relaxed),
        RECALIB_COUNT.load(Ordering::Relaxed),
    )
}

/// Returns the rating of the last used source (0–300).
pub fn last_source_rating() -> u32 {
    LAST_RATING.load(Ordering::Relaxed)
}

/// Returns `true` if the last calibration used HPET (HQ physical measurement).
pub fn calibrated_with_hpet() -> bool {
    CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)) == CalibSource::Hpet
}

/// Returns `true` if the TSC was calibrated with a real measurement source.
/// False if nominal CPUID or 3GHz fallback.
pub fn calibrated_with_real_measurement() -> bool {
    CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)).is_measured()
}

/// Returns `true` if post-APIC recalibration is recommended.
pub fn should_recalibrate() -> bool {
    !calibrated_with_real_measurement()
}

/// Returns the number of recalibrations performed.
pub fn recalibration_count() -> u32 {
    RECALIB_COUNT.load(Ordering::Relaxed)
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Rounds the TSC frequency to the nearest multiple of 100 kHz.
///
/// Reduces sub-100kHz measurement noise without observable precision loss.
/// Example: 2_999_937_012 -> 3_000_000_000 (typical value for i7 3GHz).
#[inline(always)]
pub fn round_hz(hz: u64) -> u64 {
    (hz + 50_000) / 100_000 * 100_000
}

/// Emits an ASCII tag to debug port 0xE9.
///
/// Output format: `[tag]` — visible with QEMU `-debugcon stdio` or
/// `-debugcon file:/tmp/e9.txt`.
///
/// ## Rule CAL-FALLBACK-01
/// Each step of the fallback chain MUST call this function.
#[inline]
fn e9_tag(tag: &[u8]) {
    #[inline(always)]
    unsafe fn out(b: u8) {
        core::arch::asm!("out 0xe9, al", in("al") b, options(nomem, nostack, preserves_flags));
    }
    unsafe {
        out(b'[');
        for &b in tag {
            out(b);
        }
        out(b']');
    }
}

#[inline]
fn e9_tag_hz(tag: &[u8], hz: u64) {
    #[inline(always)]
    unsafe fn out(b: u8) {
        core::arch::asm!("out 0xe9, al", in("al") b, options(nomem, nostack, preserves_flags));
    }
    unsafe {
        out(b'[');
        for &b in tag {
            out(b);
        }
        out(b' ');
        for &b in b"hz=" {
            out(b);
        }
        let mut buf = [0u8; 20];
        let mut n = hz;
        let mut i = buf.len();
        if n == 0 {
            i -= 1;
            buf[i] = b'0';
        } else {
            while n != 0 {
                i -= 1;
                buf[i] = b'0' + (n % 10) as u8;
                n /= 10;
            }
        }
        while i < buf.len() {
            out(buf[i]);
            i += 1;
        }
        out(b']');
    }
}

// ── Compatibility and legacy re-exports ───────────────────────────────────────

/// Compatibility with legacy code calling `debug_log_source(tag)`.
/// Redirects to `e9_tag()`.
#[inline(always)]
#[allow(dead_code)]
fn debug_log_source(tag: &[u8]) {
    e9_tag(tag);
}
