//! # arch/x86_64/time/calibration — TSC Calibration Module Root
//!
//! Wiring & re-exports for the TSC calibration fallback chain (`chain.rs`),
//! CPUID nominal discovery (`cpuid_nominal.rs`), validation (`validation.rs`),
//! and hardware window sampling (`window.rs`).

pub mod chain;
pub mod cpuid_nominal;
pub mod validation;
pub mod window;

pub use chain::{
    calibrate_tsc, calibrate_tsc_detail, calibrated_with_hpet, calibrated_with_real_measurement,
    calibration_stats, last_calibration_result, last_source_rating, recalibrate_tsc,
    recalibration_count, round_hz, should_recalibrate, CalibSource, CalibratedTsc,
};
pub use window::{mean_and_variance, CalibrationResult, CalibrationSample, CalibrationSource};
