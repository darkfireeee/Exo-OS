//! ExoFS Observability module (wiring facade).

pub mod alert;
pub mod debug_interface;
mod facade;
pub mod health_check;
pub mod latency_histogram;
pub mod metrics;
pub mod perf_counters;
pub mod space_tracker;
pub mod throughput_tracker;
pub mod tracing;

pub use alert::{Alert, AlertCode, AlertFilter, AlertLevel, AlertLog, AlertManager, ALERT_LOG};
pub use debug_interface::{
    DebugCommand, DebugCommandId, DebugQueue, DebugResponse, DebugResponseStatus, DebugSession,
    DebugStats, DEBUG_QUEUE,
};
pub use facade::{
    global_error_rate_ppt, health_status, is_healthy, observability_init, ObservabilityConfig,
    ObservabilityModule, ObservabilitySnapshot, ObservabilityStatus, OBSERVABILITY,
};
pub use health_check::{
    HealthCheck, HealthProbeId, HealthProbeResult, HealthProbeRing, HealthStatus, HealthThresholds,
    HEALTH,
};
pub use latency_histogram::{
    LatencyCategory, LatencyHistogram, LatencySummary, LatencyTracker, LatencyWindow, LATENCY_HIST,
    LATENCY_TRACKER,
};
pub use metrics::{
    ExofsMetrics, MetricId, MetricKind, MetricsDiff, MetricsHistory, MetricsSnapshot,
    EXOFS_METRICS, METRICS_HISTORY,
};
pub use perf_counters::{
    PerfCounterId, PerfCounterSet, PerfDelta, PerfRateWindow, PerfReport, PerfSnapshot,
    PERF_COUNTERS, PERF_RATE,
};
pub use space_tracker::{
    FragmentationInfo, SpaceHistory, SpaceQuota, SpaceSnapshot, SpaceTracker, SpaceZone,
    SpaceZoneStats, SPACE_HISTORY, SPACE_TRACKER,
};
pub use throughput_tracker::{
    ThroughputRate, ThroughputSample, ThroughputSnapshot, ThroughputThresholds, ThroughputTracker,
    ThroughputWindow, THROUGHPUT_TRACKER,
};
pub use tracing::{
    ComponentId, TraceEvent, TraceFilter, TraceLevel, TraceRing, TraceSession, TraceSummary,
    TRACE_RING,
};
