// kernel/src/security/zero_trust/mod.rs

//! Zero-Trust Security — Politique de sécurité avec vérification de chaque accès.
//!
//! PRINCIPE : "Never trust, always verify"
//! Tout accès à une ressource passe par `verify::verify_access()`.

pub mod context;
pub mod labels;
pub mod policy;
pub mod process_state;
pub mod verify;

pub use context::{ContextStats, PrincipalId, SecurityContext, TrustLevel};
pub use labels::{ConfidentialityLevel, IntegrityLevel, SecurityLabel};
pub use policy::{
    global_policy, AccessRequest, PolicyAction, PolicyStats, ResourceKind, ZeroTrustPolicy,
};
pub use process_state::{
    clear_process_restrictions, context_for_caller, inherit_restrictions, process_restrictions,
    restrict_process, trust_for_pid,
};
pub use verify::{
    register_ring1_pid, ring1_pair_trusted, ring1_trusted_mask, unregister_ring1_pid,
    verify_access, verify_crypto_key_access, verify_dma_access, verify_file_read,
    verify_file_write, verify_ipc_access, verify_ipc_access_between, verify_syscall, AccessError,
};
