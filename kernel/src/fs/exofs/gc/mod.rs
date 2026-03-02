//! GC — Garbage Collector ExoFS
//!
//! Architecture tricolore (blanc/gris/noir) + scan d'epochs + DeferredDeleteQueue.
//! RÈGLE 13 : GC n'acquiert JAMAIS EPOCH_COMMIT_LOCK.
//! RÈGLE 12 : ref_count P-Blob : panic si underflow.

#![allow(dead_code)]

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
pub mod marker;
pub mod orphan_collector;
pub mod reference_tracker;
pub mod relation_walker;
pub mod sweeper;
pub mod tricolor;

pub use blob_gc::BlobGc;
pub use blob_refcount::BlobRefcount;
pub use cycle_detector::CycleDetector;
pub use epoch_scanner::EpochScanner;
pub use gc_metrics::GcMetrics;
pub use gc_scheduler::GcScheduler;
pub use gc_state::{GcPhase, GcState};
pub use gc_thread::GcThread;
pub use gc_tuning::GcTuning;
pub use inline_gc::InlineGc;
pub use marker::Marker;
pub use orphan_collector::OrphanCollector;
pub use reference_tracker::ReferenceTracker;
pub use relation_walker::RelationWalker;
pub use sweeper::Sweeper;
pub use tricolor::{TricolorMark, TricolorSet};
