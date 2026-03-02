//! observability/ — Module d'observabilité ExoFS (no_std).

pub mod metrics;
pub mod perf_counters;
pub mod latency_histogram;
pub mod throughput_tracker;
pub mod space_tracker;
pub mod health_check;
pub mod alert;
pub mod tracing;
pub mod debug_interface;

pub use metrics::{EXOFS_METRICS, ExofsMetrics, MetricsSnapshot};
pub use perf_counters::{PERF_COUNTERS, PerfCounters};
pub use latency_histogram::{LATENCY_HIST, LatencyHistogram};
pub use throughput_tracker::{THROUGHPUT, ThroughputTracker};
pub use space_tracker::{SPACE_TRACKER, SpaceTracker};
pub use health_check::{HEALTH, HealthStatus, HealthCheck};
pub use alert::{ALERT_LOG, Alert, AlertLevel};
pub use tracing::{EXOFS_TRACER, ExofsTracer};
pub use debug_interface::DebugInterface;
