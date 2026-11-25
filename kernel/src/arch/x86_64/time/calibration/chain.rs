//! TSC calibration fallback chain, result commit, and diagnostics.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::arch::x86_64::cpu::tsc as cpu_tsc;
use crate::arch::x86_64::debug_e9_byte;

use super::super::sources;
use super::{cpuid_nominal, validation, window};

static LAST_TSC_HZ: AtomicU64 = AtomicU64::new(0);
static LAST_SOURCE: AtomicU8 = AtomicU8::new(CalibSource::None as u8);
static LAST_RATING: AtomicU32 = AtomicU32::new(0);
static CALIB_SEQ: AtomicU32 = AtomicU32::new(0);
static RECALIB_COUNT: AtomicU32 = AtomicU32::new(0);
static LAST_CONFIDENCE: AtomicU8 = AtomicU8::new(0);
static SOURCES_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Source used for TSC calibration.
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

    #[inline(always)]
    pub fn is_measured(self) -> bool {
        matches!(
            self,
            CalibSource::Hpet | CalibSource::PmTimer | CalibSource::Pit
        )
    }

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
#[derive(Clone, Copy, Debug)]
pub struct CalibratedTsc {
    pub tsc_hz: u64,
    pub source: CalibSource,
    pub confidence: u8,
    pub variance_hz2: u64,
    pub valid_samples: u8,
    pub duration_tsc_cycles: u64,
    pub tsc_invariant: bool,
    pub seq: u32,
}

impl CalibratedTsc {
    #[inline(always)]
    pub fn is_production_grade(&self) -> bool {
        self.confidence >= 80 && self.source.is_measured()
    }

    #[inline(always)]
    pub fn needs_recalibration(&self) -> bool {
        !self.source.is_measured()
    }
}

pub fn calibrate_tsc() -> u64 {
    let result = run_calibration_chain();
    commit_calibration_result(&result);
    result.tsc_hz
}

pub fn calibrate_tsc_detail() -> CalibratedTsc {
    let result = run_calibration_chain();
    commit_calibration_result(&result);
    result
}

pub fn recalibrate_tsc() -> u64 {
    let old_hz = LAST_TSC_HZ.load(Ordering::Acquire);
    RECALIB_COUNT.fetch_add(1, Ordering::Relaxed);

    let result = run_calibration_chain();

    if old_hz > 0 {
        let lo = old_hz / 2;
        let hi = old_hz.saturating_add(old_hz / 2);
        if result.tsc_hz < lo || result.tsc_hz > hi {
            e9_tag(b"RECAL-REJECT");
            return old_hz;
        }
    }

    commit_calibration_result(&result);
    result.tsc_hz
}

fn run_calibration_chain() -> CalibratedTsc {
    let seq = CALIB_SEQ.fetch_add(1, Ordering::AcqRel) + 1;
    let tsc_invariant = cpu_tsc::tsc_invariant();

    ensure_sources_initialized();
    let start_cycles = cpu_tsc::read_tsc();

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

    if let Some(hz) = cpuid_nominal::cpuid_tsc_hz() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag_hz(b"CAL:CPUID15", hz);
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Cpuid15,
                confidence: 85,
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

    if let Some(hz) = cpuid_nominal::cpuid_tsc_hz_leaf16() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag_hz(b"CAL:CPUID16", hz);
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Cpuid16,
                confidence: 70,
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

    if let Some(hz) = cpuid_nominal::cpuid_best_estimate() {
        if validation::hz_in_range(hz) {
            let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
            e9_tag_hz(b"CAL:CPUID-BEST", hz);
            return CalibratedTsc {
                tsc_hz: round_hz(hz),
                source: CalibSource::Cpuid15,
                confidence: 65,
                variance_hz2: 0,
                valid_samples: 0,
                duration_tsc_cycles: duration,
                tsc_invariant,
                seq,
            };
        }
    }

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

    let hz = cpu_tsc::calibrate_tsc_with_pit();
    if hz > 0 && validation::hz_in_range(hz) {
        let duration = cpu_tsc::read_tsc().wrapping_sub(start_cycles);
        e9_tag_hz(b"CAL:PIT-DRV", hz);
        return CalibratedTsc {
            tsc_hz: round_hz(hz),
            source: CalibSource::Pit,
            confidence: 55,
            variance_hz2: 0,
            valid_samples: 1,
            duration_tsc_cycles: duration,
            tsc_invariant,
            seq,
        };
    }
    e9_tag(b"CAL:PIT-DRV-FAIL");

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

fn commit_calibration_result(r: &CalibratedTsc) {
    cpu_tsc::set_tsc_hz(r.tsc_hz);
    sources::update_source_rating(sources::SourceId::Tsc, 400);
    LAST_TSC_HZ.store(r.tsc_hz, Ordering::Release);
    LAST_SOURCE.store(r.source as u8, Ordering::Release);
    LAST_RATING.store(r.source.rating(), Ordering::Release);
    LAST_CONFIDENCE.store(r.confidence, Ordering::Release);
}

#[inline]
fn ensure_sources_initialized() {
    if SOURCES_INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        sources::init_sources();
    }
}

pub fn last_calibration_result() -> Option<CalibratedTsc> {
    let hz = LAST_TSC_HZ.load(Ordering::Acquire);
    if hz == 0 {
        return None;
    }
    let seq = CALIB_SEQ.load(Ordering::Relaxed);
    let source = CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed));
    Some(CalibratedTsc {
        tsc_hz: hz,
        source,
        confidence: LAST_CONFIDENCE.load(Ordering::Relaxed),
        variance_hz2: 0,
        valid_samples: if source.is_measured() {
            window::N_SAMPLES as u8
        } else {
            0
        },
        duration_tsc_cycles: 0,
        tsc_invariant: cpu_tsc::tsc_invariant(),
        seq,
    })
}

pub fn calibration_stats() -> (u64, u32, u8, u32) {
    (
        LAST_TSC_HZ.load(Ordering::Relaxed),
        CALIB_SEQ.load(Ordering::Relaxed),
        LAST_CONFIDENCE.load(Ordering::Relaxed),
        RECALIB_COUNT.load(Ordering::Relaxed),
    )
}

pub fn last_source_rating() -> u32 {
    LAST_RATING.load(Ordering::Relaxed)
}

pub fn calibrated_with_hpet() -> bool {
    CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)) == CalibSource::Hpet
}

pub fn calibrated_with_real_measurement() -> bool {
    CalibSource::from_u8(LAST_SOURCE.load(Ordering::Relaxed)).is_measured()
}

pub fn should_recalibrate() -> bool {
    !calibrated_with_real_measurement()
}

pub fn recalibration_count() -> u32 {
    RECALIB_COUNT.load(Ordering::Relaxed)
}

#[inline(always)]
pub fn round_hz(hz: u64) -> u64 {
    (hz + 50_000) / 100_000 * 100_000
}

#[inline]
fn e9_tag(tag: &[u8]) {
    debug_e9_byte(b'[');
    for &b in tag {
        debug_e9_byte(b);
    }
    debug_e9_byte(b']');
}

#[inline]
fn e9_tag_hz(tag: &[u8], hz: u64) {
    debug_e9_byte(b'[');
    for &b in tag {
        debug_e9_byte(b);
    }
    for &b in b" hz=" {
        debug_e9_byte(b);
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
        debug_e9_byte(buf[i]);
        i += 1;
    }
    debug_e9_byte(b']');
}
