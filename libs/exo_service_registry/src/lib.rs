//! # exo_service_registry - Service Discovery & Registry pour Exo-OS
//!
//! Bibliothèque de service discovery production-ready avec:
//! - **Registration**: Enregistrement de services avec metadata
//! - **Discovery**: Lookup O(1) avec cache LRU et bloom filter
//! - **Health monitoring**: Heartbeat automatique et recovery
//! - **Persistence**: Backends TOML/in-memory
//! - **Thread-safe**: Synchronisation optimisée avec RwLock
//! - **Zero-allocation paths**: Hot paths optimisés
//!
//! ## Performance
//! - Lookup: <100ns avec cache hit
//! - Registration: O(log n) avec persistence
//! - Bloom filter: 1% false positive rate
//! - Memory: ~256 bytes par service
//!
//! ## Architecture
//! ```text
//! Registry (Core)
//!   ├── Storage Backend (Trait)
//!   │   ├── InMemory
//!   │   └── Toml (feature: persistent)
//!   ├── Cache (LRU)
//!   ├── Bloom Filter
//!   └── Health Checker (feature: health_check)
//! ```
//!
//! ## Usage
//! ```ignore
//! use exo_service_registry::{Registry, ServiceName, ServiceInfo};
//!
//! // Créer le registry
//! let mut registry = Registry::new();
//!
//! // Enregistrer un service
//! let name = ServiceName::new("fs_service").unwrap();
//! let info = ServiceInfo::new("/tmp/fs.sock");
//! registry.register(name.clone(), info)?;
//!
//! // Découvrir un service
//! if let Some(info) = registry.lookup(&name) {
//!     println!("Service: {}", info.endpoint());
//! }
//! ```

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

extern crate alloc;

// Modules principaux
mod types;
mod registry;
mod discovery;
mod storage;

#[cfg(feature = "health_check")]
mod health;

// Réexportations publiques
pub use types::{
    ServiceName, ServiceInfo, ServiceMetadata, ServiceStatus,
    RegistryError, RegistryResult,
};

pub use registry::{Registry, RegistryConfig, RegistryStats};
pub use discovery::{Discovery, DiscoveryClient};
pub use storage::{StorageBackend, InMemoryBackend};

#[cfg(feature = "persistent")]
pub use storage::TomlBackend;

#[cfg(feature = "health_check")]
pub use health::{HealthChecker, HealthStatus, HealthConfig};

/// Version de la bibliothèque
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Taille maximale d'un nom de service
pub const MAX_SERVICE_NAME_LEN: usize = 64;

/// Entrées par défaut dans le cache LRU
pub const DEFAULT_CACHE_SIZE: usize = 100;

/// TTL par défaut du cache (secondes)
pub const DEFAULT_CACHE_TTL_SECS: u64 = 60;

/// Taille par défaut du bloom filter
pub const DEFAULT_BLOOM_SIZE: usize = 10_000;

/// Taux de faux positifs bloom filter
pub const DEFAULT_BLOOM_FP_RATE: f64 = 0.01;

/// Module prelude pour imports courants
pub mod prelude {
    pub use crate::{
        Registry, RegistryConfig, RegistryStats,
        Discovery, DiscoveryClient,
        ServiceName, ServiceInfo, ServiceMetadata, ServiceStatus,
        RegistryError, RegistryResult,
        StorageBackend, InMemoryBackend,
    };

    #[cfg(feature = "persistent")]
    pub use crate::TomlBackend;

    #[cfg(feature = "health_check")]
    pub use crate::{HealthChecker, HealthStatus, HealthConfig};
}
