// kernel/src/security/isolation/mod.rs
//
// Module isolation — Isolation de processus et domaines de sécurité

pub mod domains;
pub mod namespaces;
pub mod sandbox;
pub mod pledge;

pub use domains::{
    SecurityDomain, DomainContext, DomainError,
    domain_flags, read_domain_stats, DomainStatsSnapshot,
};
pub use namespaces::{
    NsId, NsKind, Namespace, NamespaceSet,
    create_namespace, destroy_namespace, NsError, ns_flags,
};
pub use sandbox::{
    SandboxPolicy, SandboxAction, syscall_nr,
    record_sandbox_decision, sandbox_global_stats, SandboxGlobalStats,
};
pub use pledge::{
    PledgeSet, PledgeError, pledge_flags,
    global_pledge_violations,
};
