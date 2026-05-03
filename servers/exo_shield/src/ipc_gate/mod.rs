//! # ipc_gate — IPC policy enforcement & audit logging
//!
//! The IPC gate sits between processes and the IPC router, enforcing
//! allow/deny rules for inter-process communication and maintaining
//! a comprehensive audit log of all IPC transactions.
//!
//! ## Modules
//! - `policy` — Policy table & evaluation engine
//! - `audit`  — Audit ring buffer & query/export

pub mod access;
pub mod audit;
pub mod policy;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use policy::{
    add_policy, enumerate_policies, evaluate_policy, get_default_policy, get_policy_stats,
    lookup_policy, policy_init, remove_policy, set_default_policy, PolicyAction, PolicyEvalResult,
    PolicyRule, PolicyStats, PolicyTable,
};

pub use audit::{
    audit_init, export_audit, get_audit_stats, query_audit, query_audit_by_time,
    query_audit_filtered, query_audit_pair, record_audit, AuditEntry, AuditFilter, AuditResult,
    AuditRingBuffer, AuditStats,
};

pub use access::{
    classify_service_cap_requirement, ServiceCapRequirement, EXO_SHIELD_CAP_TOKEN_LEN,
    EXO_SHIELD_CAP_TOKEN_OFFSET,
};
