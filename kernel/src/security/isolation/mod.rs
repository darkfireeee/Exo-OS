//! Process Isolation
//!
//! Sandbox and isolation primitives

pub mod container;
pub mod namespace;
pub mod sandbox;
pub mod seccomp;

pub use container::{Container, ContainerConfig};
pub use namespace::{Namespace, NamespaceType};
pub use sandbox::{Sandbox, SandboxPolicy};
pub use seccomp::{SeccompAction, SeccompFilter};

/// Initialize isolation subsystem
pub fn init() {
    log::info!("Isolation subsystem initialized");
}
