// kernel/src/security/isolation/mod.rs
//
// Module isolation — Isolation de processus et domaines de sécurité

pub mod domains;
pub mod namespaces;
pub mod pledge;
pub mod sandbox;

pub use domains::{
    domain_flags, read_domain_stats, DomainContext, DomainError, DomainStatsSnapshot,
    SecurityDomain,
};
pub use namespaces::{
    create_namespace, destroy_namespace, ns_flags, Namespace, NamespaceSet, NsError, NsId, NsKind,
};
pub use pledge::{global_pledge_violations, pledge_flags, PledgeError, PledgeSet};
pub use sandbox::{
    record_sandbox_decision, sandbox_global_stats, syscall_nr, SandboxAction, SandboxGlobalStats,
    SandboxPolicy,
};
