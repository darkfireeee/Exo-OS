// kernel/src/security/zero_trust/mod.rs

//! Zero-Trust Security — Politique de sécurité avec vérification de chaque accès.
//!
//! PRINCIPE : "Never trust, always verify"
//! Tout accès à une ressource passe par `verify::verify_access()`.

pub mod context;
pub mod labels;
pub mod policy;
pub mod verify;

pub use context::{SecurityContext, TrustLevel, PrincipalId, ContextStats};
pub use labels::{SecurityLabel, ConfidentialityLevel, IntegrityLevel};
pub use policy::{
    ZeroTrustPolicy, PolicyAction, ResourceKind, AccessRequest,
    PolicyStats, global_policy,
};
pub use verify::{
    AccessError, verify_access, verify_file_read, verify_file_write,
    verify_ipc_access, verify_crypto_key_access, verify_dma_access, verify_syscall,
};
