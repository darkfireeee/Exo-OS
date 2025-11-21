//! # Services Framework - Infrastructure Commune pour Services Exo-OS
//!
//! Framework fournissant des abstractions communes pour tous les services userspace.
//! Simplifie la création de nouveaux services avec:
//! - Trait Service standardisé
//! - Registration auprès d'init
//! - Discovery de services
//! - Helpers IPC (Request/Response, Pub/Sub)
//!
//! ## Utilisation
//!
//! ```rust
//! use services::{Service, ServiceRegistry};
//!
//! struct MyService {
//!     // ...
//! }
//!
//! impl Service for MyService {
//!     fn name(&self) -> &str { "my_service" }
//!     fn start(&mut self) -> Result<()> { Ok(()) }
//!     // ...
//! }
//!
//! fn main() {
//!     let service = MyService::new();
//!     ServiceRegistry::register(service)?;
//! }
//! ```

#![no_std]

extern crate alloc;

pub mod discovery;
pub mod ipc_helpers;
pub mod registry;
pub mod service;

// Réexportations principales
pub use discovery::ServiceDiscovery;
pub use ipc_helpers::{RequestResponseClient, RequestResponseServer};
pub use registry::ServiceRegistry;
pub use service::{HealthStatus, Service, ServiceCapabilities};

use exo_types::Result;

/// Initialise le framework services
pub fn init() -> Result<()> {
    log::debug!("Services framework initialized");
    Ok(())
}

/// Version du framework
pub const SERVICES_VERSION: &str = "0.1.0";
