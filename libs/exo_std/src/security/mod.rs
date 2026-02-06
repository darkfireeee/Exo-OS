//! Système de sécurité basé sur les capabilities
//!
//! Ce module réexporte les types de capabilities depuis exo_types
//! pour fournir une API unifiée de sécurité.

// Réexportation des types canoniques depuis exo_types
pub use exo_types::capability::{Capability, CapabilityType, Rights};
pub use exo_types::capability::{CapabilityMetadata, MetadataFlags};

// Note: Les fonctions verify_capability, request_capability, etc.
// sont maintenant des syscalls dans exo_std::syscall::security
