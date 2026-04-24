pub mod core;
pub mod realtime;
pub mod scanner;

// ── Re-export commonly used types from core ─────────────────────────────────

pub use core::{
    active_threat_count, assess_pid, compute_threat_score, contain_threat, containment_for_level,
    core_init, core_is_init, count_threats_at_level, get_core_stats, get_risk_profile, get_threat,
    get_threats_by_pid, mark_process_contained, record_threat, release_process, resolve_threat,
    score_to_level, stat_assessments_inc, stat_containments_inc, stat_critical_inc,
    stat_resolved_inc, stat_threats_inc, update_risk_profile, ContainmentAction, CoreStats,
    ProcessRiskProfile, ThreatAssessment, ThreatCategory, ThreatLevel, ThreatRecord, MAX_SIG_NAME,
    MAX_THREAT_RECORDS, MAX_TRACKED_PROCESSES,
};

// ── Re-export commonly used types from scanner ──────────────────────────────

pub use scanner::{
    active_scan_profile, add_signature, complete_scan_request, compute_entropy, disable_signature,
    enable_signature, execute_scan, get_latest_scan_result, get_scan_profile, get_scan_result,
    get_scanner_stats, heuristic_analyze, next_scan_request, pending_scan_count,
    periodic_scan_tick, queue_scan, register_periodic_scan, scanner_init, scanner_is_init,
    set_scan_profile, stat_scan_executed, stat_scan_queued, store_scan_result,
    unregister_periodic_scan, ScanProfile, ScanRequest, ScanResult, ScannerStats, SignatureEntry,
    DEFAULT_SCAN_INTERVAL_TICKS, MAX_SIGNATURES, SCAN_QUEUE_MAX,
};

// ── Re-export commonly used types from realtime ─────────────────────────────

pub use realtime::{
    acknowledge_alert, add_event_filter, generate_manual_alert, get_alert, get_alerts_for_pid,
    get_process_rate, get_realtime_stats, is_process_monitored, monitor_process, realtime_init,
    realtime_is_init, remove_event_filter, set_process_rate_limits, submit_event,
    unacknowledged_alert_count, unmonitor_process, Alert, EventFilter, EventProcessResult,
    EventType, FilterAction, MonitoredEvent, RateEntry, RealtimeStats, MAX_ALERTS,
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
