// kernel/src/arch/x86_64/time/drift/mod.rs
//
// TSC drift correction sub-module.
//
// This module implements the long-term correction mechanism for TSC drift
// (crystal aging, thermal variation, P-state migration).
//
// Architecture:
//   pll.rs       — Software PLL: smooths corrections ±500 ppm max (RULE DRIFT-PLL-01)
//   periodic.rs  — Periodic recalibration thread (RULE DRIFT-PREEMPT-01 / DRIFT-CIRCULAR-01)
//
// Usage:
//   Called from time_init() to initialize, then drift_tick() from tick.rs.

pub mod periodic;
pub mod pll;

// Public re-exports for time_init() and tick.rs.
pub use periodic::{
    drift_fail_count, drift_init, drift_last_applied_hz, drift_last_measured_hz,
    drift_monotone_fixes, drift_recal_count, drift_snapshot, drift_tick, update_cpu_load,
    DriftSnapshot,
};
pub use pll::{
    pll_correction_count, pll_current_hz, pll_init, pll_last_adj_hz, pll_locked, pll_snapshot,
    pll_update, PllSnapshot,
};
