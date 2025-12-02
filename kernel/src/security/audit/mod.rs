//! Audit Subsystem
//!
//! Security event logging and analysis

pub mod analyzer;
pub mod logger;

pub use analyzer::{
    analyze_events, detect_brute_force, detect_time_anomaly, get_statistics, get_top_offenders,
    AuditStatistics, ThreatLevel,
};
pub use logger::{
    audit_log, get_all_events, get_audit_summary, AuditEvent, AuditEventType, AuditLog,
    AuditSummary,
};
