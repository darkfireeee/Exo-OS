//! Monitoring - Filesystem Observability
//!
//! ## Modules
//! - `notify`: File change notifications (inotify/fanotify)
//! - `metrics`: Performance metrics collection
//! - `trace`: Debug tracing
//! - `profiler`: Performance profiling
//!
//! ## Features
//! - Real-time file change notifications
//! - Performance metrics tracking
//! - Debug event tracing

pub mod notify;
pub mod metrics;
pub mod trace;
pub mod profiler;

/// Initialize monitoring subsystem
pub fn init() {
    log::info!("Initializing filesystem monitoring");

    notify::init();
    metrics::init();
    trace::init();
    profiler::init();

    log::info!("✓ Filesystem monitoring initialized");
}
