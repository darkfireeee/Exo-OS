// kernel/src/security/audit/mod.rs
//
// Module audit — Journal d'audit de politique de sécurité
//
// Sous-modules :
//   • logger        — Ring buffer d'événements, filtres, flush vers userspace
//   • rules         — RuleSet 64 entrées, évaluation par priorité
//   • syscall_audit — Intégration SYSCALL entry/exit, verdict par thread

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

/// Initialise le sous-système d'audit.
///
/// Installe les règles par défaut :
///   - Log tous les syscalls (priorité 128)
///   - Alert sur SecurityViolation (couverte par RÈGLE AUDIT-02 — toujours active)
pub fn audit_init() {
    let _ = add_global_rule(AuditRule::new_log_all());
}
