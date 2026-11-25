//! Module GC ExoFS — Garbage Collector Tricolore (façade de câblage).

pub mod blob_gc;
pub mod blob_refcount;
pub mod cycle_detector;
pub mod epoch_scanner;
pub mod gc_metrics;
pub mod gc_scheduler;
pub mod gc_state;
pub mod gc_thread;
pub mod gc_tuning;
pub mod inline_gc;
mod lifecycle;
pub mod marker;
pub mod orphan_collector;
pub mod reference_tracker;
pub mod relation_walker;
pub mod sweeper;
pub mod tricolor;

pub use blob_gc::{BlobGcConfig, GcPassResult, BLOB_GC};
pub use blob_refcount::{BLOB_REFCOUNT, GC_MIN_DEFERRED_EPOCHS};
pub use cycle_detector::{CycleDetectStats, DetectedCycle, CYCLE_DETECTOR};
pub use epoch_scanner::{BlobLookup, EmptyBlobLookup, EpochScanSnapshot, ScanStats, EPOCH_SCANNER};
pub use gc_metrics::{GcMetricsSnapshot, GC_METRICS};
pub use gc_scheduler::{GcSchedulerStats, ScheduleDecision, ScheduleReason, GC_SCHEDULER};
pub use gc_state::{GcPassStats, GcPhase, GcStateSnapshot, GC_STATE};
pub use gc_thread::{gc_thread_entry, GcThread, GcThreadControl, GcThreadStats, GC_THREAD};
pub use gc_tuning::{GcSystemState, GcTriggerReason, GcTuningParams, GC_TUNER};
pub use inline_gc::{
    InlineGcResult, InlineGcStats, InlineObjectEntry, INLINE_DATA_THRESHOLD, INLINE_GC,
};
pub use lifecycle::{
    gc_force_pass, gc_init, gc_metrics_snapshot, gc_shutdown, gc_state_snapshot,
};
pub use marker::{MarkerConfig, MarkingResult, MARKER};
pub use orphan_collector::{OrphanResult, ORPHAN_COLLECTOR};
pub use reference_tracker::{ReferenceTracker, REFERENCE_TRACKER};
pub use relation_walker::{RelationEdge, WalkStats, RELATION_WALKER};
pub use sweeper::{SweepConfig, SweeperResult, SWEEPER};
pub use tricolor::{
    BlobNode, MarkStats, SweepResult, TriColor, TricolorWorkspace, GC_MARK_BATCH_SIZE,
    MAX_GC_GREY_QUEUE,
};
