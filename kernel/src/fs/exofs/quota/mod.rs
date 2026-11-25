//! ExoFS Quota — Main module (wiring facade).

mod facade;
pub mod quota_audit;
pub mod quota_enforcement;
pub mod quota_namespace;
pub mod quota_policy;
pub mod quota_report;
pub mod quota_tracker;

pub use facade::{
    quota_check_write, quota_init, quota_set_limits, QuotaConfig, QuotaModule, QuotaModuleState,
    QUOTA,
};
pub use quota_audit::{
    advance_audit_tick, audit_tick, AuditFilter, AuditSession, AuditSummary, QuotaAuditEntry,
    QuotaAuditLog, QuotaEvent, QUOTA_AUDIT,
};
pub use quota_enforcement::{
    EnforcementAction, EnforcementResult, EnforcementStats, QuotaEnforcer, QuotaViolation,
    QUOTA_ENFORCER,
};
pub use quota_namespace::{
    NamespaceFlags, NamespaceId, NamespaceStats, QuotaNamespace, QuotaNamespaceEntry,
    NAMESPACE_MAX, NS_NAME_LEN, QUOTA_NAMESPACE,
};
pub use quota_policy::{PolicyFlags, PolicyPresets, QuotaKind, QuotaLimits, QuotaPolicy};
pub use quota_report::{
    QuotaReport, QuotaReportEntry, QuotaReporter, ReportDimension, QUOTA_REPORTER,
    SEVERITY_CRITICAL_PPT, SEVERITY_WARNING_PPT,
};
pub use quota_tracker::{
    QuotaEntry, QuotaKey, QuotaTracker, QuotaUsage, QUOTA_MAX_ENTRIES, QUOTA_TRACKER,
};
