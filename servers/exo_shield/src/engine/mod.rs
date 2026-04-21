pub mod core;
pub mod scanner;
pub mod realtime;

// ── Re-export commonly used types from core ─────────────────────────────────

pub use core::{
    ThreatLevel,
    ThreatCategory,
    ThreatRecord,
    ThreatAssessment,
    ContainmentAction,
    ProcessRiskProfile,
    CoreStats,
    MAX_THREAT_RECORDS,
    MAX_SIG_NAME,
    MAX_TRACKED_PROCESSES,
    compute_threat_score,
    score_to_level,
    containment_for_level,
    record_threat,
    get_threat,
    get_threats_by_pid,
    contain_threat,
    resolve_threat,
    count_threats_at_level,
    active_threat_count,
    assess_pid,
    update_risk_profile,
    get_risk_profile,
    mark_process_contained,
    release_process,
    core_init,
    core_is_init,
    get_core_stats,
    stat_assessments_inc,
    stat_threats_inc,
    stat_containments_inc,
    stat_resolved_inc,
    stat_critical_inc,
};

// ── Re-export commonly used types from scanner ──────────────────────────────

pub use scanner::{
    SignatureEntry,
    ScanResult,
    ScanRequest,
    ScanProfile,
    ScannerStats,
    SCAN_QUEUE_MAX,
    MAX_SIGNATURES,
    DEFAULT_SCAN_INTERVAL_TICKS,
    compute_entropy,
    heuristic_analyze,
    execute_scan,
    add_signature,
    disable_signature,
    enable_signature,
    queue_scan,
    next_scan_request,
    complete_scan_request,
    store_scan_result,
    get_scan_result,
    get_latest_scan_result,
    set_scan_profile,
    get_scan_profile,
    active_scan_profile,
    pending_scan_count,
    register_periodic_scan,
    unregister_periodic_scan,
    periodic_scan_tick,
    scanner_init,
    scanner_is_init,
    stat_scan_executed,
    stat_scan_queued,
    get_scanner_stats,
};

// ── Re-export commonly used types from realtime ─────────────────────────────

pub use realtime::{
    EventType,
    MonitoredEvent,
    EventFilter,
    FilterAction,
    RateEntry,
    Alert,
    EventProcessResult,
    RealtimeStats,
    MAX_ALERTS,
    submit_event,
    monitor_process,
    unmonitor_process,
    is_process_monitored,
    set_process_rate_limits,
    get_process_rate,
    add_event_filter,
    remove_event_filter,
    get_alert,
    get_alerts_for_pid,
    acknowledge_alert,
    unacknowledged_alert_count,
    generate_manual_alert,
    realtime_init,
    realtime_is_init,
    get_realtime_stats,
};

// ── Unified Engine Init ─────────────────────────────────────────────────────

/// Initialize all engine sub-modules in correct order.
pub fn engine_init() {
    core_init();
    scanner_init();
    realtime_init();
}

/// Check if all engine sub-modules are initialized.
pub fn engine_is_init() -> bool {
    core_is_init() && scanner_is_init() && realtime_is_init()
}
