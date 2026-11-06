// kernel/src/security/audit/mod.rs
//
// Audit module — Security policy audit log
//
// Submodules:
//   • logger        — Event ring buffer, filters, flush to userspace
//   • rules         — 64-entry RuleSet, evaluation by priority
//   • syscall_audit — SYSCALL entry/exit integration, per-thread verdict

pub mod logger;
pub mod rules;
pub mod syscall_audit;

pub use logger::{
    audit_logger_stats, flush_to_userspace, log_event, log_security_violation, pending_events,
    set_filter, AuditCategory, AuditOutcome, AuditRecord,
};

pub use rules::{
    add_global_rule, evaluate_global, remove_global_rule, rule_stats, AuditRule, RuleAction,
};

pub use syscall_audit::{
    audit_capability_deny, audit_file_deny, audit_syscall_entry, audit_syscall_exit,
    syscall_audit_stats, AuditVerdict,
};

/// Initializes the audit subsystem.
///
/// Installs default rules:
///   - Log all syscalls (priority 128)
///   - Alert on SecurityViolation (covered by RULE AUDIT-02 — always active)
pub fn audit_init() {
    let _ = add_global_rule(AuditRule::new_log_all());
}
