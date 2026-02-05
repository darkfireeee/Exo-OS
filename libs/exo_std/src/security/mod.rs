//! Système de sécurité basé sur les capabilities

pub mod capability;

// Réexportations
pub use capability::{Capability, CapabilityType, Rights};
pub use capability::{verify_capability, check_rights};
pub use capability::{request_capability, revoke_capability, delegate_capability};
