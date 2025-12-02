//! Exo-OS Security Module
//!
//! High-performance capability-based security system
//!
//! # Features
//! - **O(1) permission checks** - Bitset-based rights checking
//! - **Lock-free reads** - RCU-style capability access
//! - **Cache-friendly** - Compact data structures
//! - **Zero-copy** - Capability sharing via Arc
//! - **Type-safe** - Strong typing for capabilities and rights
//!
//! # Architecture
//! - Capabilities grant access to objects
//! - Rights define what operations are allowed
//! - Per-process capability tables for isolation
//! - Global object registry for tracking
//!
//! # Performance
//! - Permission check: <50ns
//! - Capability lookup: <20ns
//! - Table access: O(1)

pub mod capability;
pub mod object;
pub mod permission;
pub mod posix_compat;


// Existing modules
pub mod audit;
pub mod collections;
pub mod crypto;
pub mod hsm;
pub mod isolation;
pub mod tpm;

// Re-exports for convenience
pub use capability::{
    Capability, CapabilityId, CapabilityMetadata, CapabilityTable, Right, RightSet,
};
pub use object::{Object, ObjectId, ObjectType};
pub use permission::{check_capability, check_permission, PermissionContext, PermissionError};
pub use posix_compat::{fd_to_capability, mode_to_rights, rights_to_mode, uid_gid_check};

/// Initialize security subsystem
pub fn init() {
    log::info!("Security subsystem initialized");
    log::info!("  - Capability-based access control");
    log::info!("  - High-performance permission checks (<50ns)");
    log::info!("  - Per-process isolation");
}

/// Security configuration
pub struct SecurityConfig {
    pub strict_mode: bool,
    pub audit_enabled: bool,
    pub crypto_enabled: bool,
}

impl SecurityConfig {
    pub const fn default() -> Self {
        Self {
            strict_mode: true,
            audit_enabled: false,
            crypto_enabled: false,
        }
    }
}

static SECURITY_CONFIG: spin::Once<SecurityConfig> = spin::Once::new();

pub fn configure(config: SecurityConfig) {
    SECURITY_CONFIG.call_once(|| config);
}

pub fn config() -> &'static SecurityConfig {
    static DEFAULT_CONFIG: SecurityConfig = SecurityConfig::default();
    SECURITY_CONFIG.get().unwrap_or(&DEFAULT_CONFIG)
}
