//! # ipc_gate — IPC policy enforcement & audit logging
//!
//! The IPC gate sits between processes and the IPC router, enforcing
//! allow/deny rules for inter-process communication and maintaining
//! a comprehensive audit log of all IPC transactions.
//!
//! ## Modules
//! - `policy` — Policy table & evaluation engine
//! - `audit`  — Audit ring buffer & query/export

pub mod policy;
pub mod audit;

// ── Re-exports ────────────────────────────────────────────────────────────────

pub use policy::{
    PolicyRule, PolicyAction, PolicyTable, PolicyEvalResult, PolicyStats,
    evaluate_policy, add_policy, remove_policy, lookup_policy,
    set_default_policy, get_default_policy, enumerate_policies,
    get_policy_stats, policy_init,
};

pub use audit::{
    AuditEntry, AuditResult, AuditRingBuffer, AuditFilter, AuditStats,
    record_audit, query_audit, query_audit_filtered, export_audit,
    query_audit_by_time, query_audit_pair,
    get_audit_stats, audit_init,
};
