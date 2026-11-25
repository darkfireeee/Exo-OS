//! ExoFS GC lifecycle helpers (`gc_init`, `gc_shutdown`, `gc_force_pass`, snapshots).
//!
//! Extracted from `exofs/gc/mod.rs` so `mod.rs` remains a pure wiring facade.

use super::{
    GcMetricsSnapshot, GcStateSnapshot, ScheduleReason, GC_METRICS, GC_SCHEDULER, GC_STATE,
    GC_THREAD, GC_TUNER,
};

pub fn gc_init() {
    GC_SCHEDULER.set_enabled(true);
    GC_TUNER.validate_params().ok();
    GC_SCHEDULER.force_trigger(ScheduleReason::Bootstrap);
}

pub fn gc_shutdown() {
    GC_THREAD.shutdown();
    GC_SCHEDULER.set_enabled(false);
}

pub fn gc_force_pass() {
    GC_THREAD.trigger_urgent();
}

pub fn gc_metrics_snapshot() -> GcMetricsSnapshot {
    GC_METRICS.snapshot()
}

pub fn gc_state_snapshot() -> GcStateSnapshot {
    GC_STATE.snapshot()
}
