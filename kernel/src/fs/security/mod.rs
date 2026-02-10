//! Security - Filesystem Security Features
//!
//! ## Modules
//! - `permissions`: Permission checking (rwx)
//! - `capabilities`: Linux capabilities
//! - `namespace`: Mount namespaces
//! - `quota`: Disk quotas
//! - `selinux`: SELinux labels (placeholder)
//!
//! ## Features
//! - POSIX permissions (rwx)
//! - Linux capabilities
//! - Mount namespace isolation
//! - Disk quota management

pub mod permissions;
pub mod capabilities;
pub mod namespace;
pub mod quota;
pub mod selinux;

/// Initialize security subsystem
pub fn init() {
    log::info!("Initializing filesystem security");

    namespace::init();
    quota::init();
    permissions::init();
    capabilities::init();

    log::info!("✓ Filesystem security initialized");
}
