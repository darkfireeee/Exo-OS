// kernel/src/arch/x86_64/time/drift/mod.rs
//
// Sous-module de correction de dérive TSC.
//
// Ce module implémente le mécanisme de correction long terme de la dérive
// du TSC (crystal aging, variation thermique, migration P-state).
//
// Architecture :
//   pll.rs       — Software PLL : lisse les corrections ±500 ppm max (RÈGLE DRIFT-PLL-01)
//   periodic.rs  — Thread de recalibration périodique (RÈGLE DRIFT-PREEMPT-01 / DRIFT-CIRCULAR-01)
//
// Usage :
//   Appelé depuis time_init() pour initialiser, puis drift_tick() depuis tick.rs.

pub mod periodic;
pub mod pll;

// Ré-exports publics pour time_init() et tick.rs.
pub use periodic::{
    drift_fail_count, drift_init, drift_last_applied_hz, drift_last_measured_hz,
    drift_monotone_fixes, drift_recal_count, drift_snapshot, drift_tick, update_cpu_load,
    DriftSnapshot,
};
pub use pll::{
    pll_correction_count, pll_current_hz, pll_init, pll_last_adj_hz, pll_locked, pll_snapshot,
    pll_update, PllSnapshot,
};
