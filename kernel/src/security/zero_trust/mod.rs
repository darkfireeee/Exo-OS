// kernel/src/security/zero_trust/mod.rs

//! Zero-Trust Security — Politique de sécurité avec vérification de chaque accès.
//!
//! PRINCIPE : "Never trust, always verify"
//! Tout accès à une ressource passe par `verify::verify_access()`.

pub mod context;
pub mod labels;
pub mod policy;
pub mod verify;

pub use context::{ContextStats, PrincipalId, SecurityContext, TrustLevel};
pub use labels::{ConfidentialityLevel, IntegrityLevel, SecurityLabel};
pub use policy::{
    global_policy, AccessRequest, PolicyAction, PolicyStats, ResourceKind, ZeroTrustPolicy,
};
pub use verify::{
    verify_access, verify_crypto_key_access, verify_dma_access, verify_file_read,
    verify_file_write, verify_ipc_access, verify_syscall, AccessError,
};
