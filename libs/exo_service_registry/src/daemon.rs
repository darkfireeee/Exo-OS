//! Registry daemon - Serveur IPC pour service registry
//!
//! Le daemon écoute les requêtes IPC et gère le registry de manière centralisée.
//!
//! Architecture:
//! ```text
//! Clients (N) → [IPC channels] → Daemon → Registry (shared)
//! ```
//!
//! ## Usage
//!
//! ```ignore
//! use exo_service_registry::daemon::RegistryDaemon;
//!
//! let daemon = RegistryDaemon::new(registry)?;
//! daemon.run(); // Bloque et traite les requêtes
//! ```

use alloc::boxed::Box;
use core::sync::atomic::Ordering;

use crate::protocol::{
    RegistryRequest, RegistryResponse, RequestType, RegistryStatsData,
};
use crate::registry::Registry;
use crate::types::RegistryResult;

/// Configuration du daemon
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Nombre maximal de connexions simultanées
    pub max_connections: usize,

    /// Taille du buffer de requêtes en attente
    pub request_queue_size: usize,

    /// Timeout pour une requête (millisecondes)
    pub request_timeout_ms: u64,

    /// Activer le mode verbose (logging)
    pub verbose: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            max_connections: 100,
            request_queue_size: 256,
            request_timeout_ms: 5000,
            verbose: false,
        }
    }
}

impl DaemonConfig {
    /// Crée une nouvelle configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Définit le nombre max de connexions
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Définit la taille de la queue
    pub fn with_queue_size(mut self, size: usize) -> Self {
        self.request_queue_size = size;
        self
    }

    /// Active le mode verbose
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }
}

/// Registry daemon IPC
///
/// Gère un registry de manière centralisée et répond aux requêtes IPC.
pub struct RegistryDaemon {
    /// Registry sous-jacent
    registry: Box<Registry>,

    /// Configuration
    config: DaemonConfig,

    /// Compteur de requêtes traitées
    requests_processed: u64,
}

impl RegistryDaemon {
    /// Crée un nouveau daemon avec registry par défaut
    pub fn new() -> Self {
        Self::with_registry(Box::new(Registry::new()))
    }

    /// Crée un daemon avec un registry custom
    pub fn with_registry(registry: Box<Registry>) -> Self {
        Self {
            registry,
            config: DaemonConfig::default(),
            requests_processed: 0,
        }
    }

    /// Crée avec configuration custom
    pub fn with_config(registry: Box<Registry>, config: DaemonConfig) -> Self {
        Self {
            registry,
            config,
            requests_processed: 0,
        }
    }

    /// Traite une requête et retourne une réponse
    ///
    /// Cette fonction est le cœur du daemon - elle dispatch les requêtes
    /// vers les bonnes méthodes du registry.
    pub fn handle_request(&mut self, request: RegistryRequest) -> RegistryResponse {
        self.requests_processed += 1;

        match request.request_type {
            RequestType::Register => self.handle_register(request),
            RequestType::Lookup => self.handle_lookup(request),
            RequestType::Unregister => self.handle_unregister(request),
            RequestType::Heartbeat => self.handle_heartbeat(request),
            RequestType::List => self.handle_list(),
            RequestType::ListByStatus => self.handle_list_by_status(request),
            RequestType::GetStats => self.handle_get_stats(),
            RequestType::Ping => RegistryResponse::pong(),
        }
    }

    /// Traite Register
    fn handle_register(&mut self, request: RegistryRequest) -> RegistryResponse {
        let name = match request.service_name {
            Some(n) => n,
            None => return RegistryResponse::error("missing service name".into()),
        };

        let info = match request.service_info {
            Some(i) => i,
            None => return RegistryResponse::error("missing service info".into()),
        };

        match self.registry.register(name, info) {
            Ok(()) => RegistryResponse::ok(),
            Err(e) => RegistryResponse::from_error(e),
        }
    }

    /// Traite Lookup
    fn handle_lookup(&mut self, request: RegistryRequest) -> RegistryResponse {
        let name = match request.service_name {
            Some(n) => n,
            None => return RegistryResponse::error("missing service name".into()),
        };

        match self.registry.lookup(&name) {
            Some(info) => RegistryResponse::found(info),
            None => RegistryResponse::not_found(),
        }
    }

    /// Traite Unregister
    fn handle_unregister(&mut self, request: RegistryRequest) -> RegistryResponse {
        let name = match request.service_name {
            Some(n) => n,
            None => return RegistryResponse::error("missing service name".into()),
        };

        match self.registry.unregister(&name) {
            Ok(()) => RegistryResponse::ok(),
            Err(e) => RegistryResponse::from_error(e),
        }
    }

    /// Traite Heartbeat
    fn handle_heartbeat(&mut self, request: RegistryRequest) -> RegistryResponse {
        let name = match request.service_name {
            Some(n) => n,
            None => return RegistryResponse::error("missing service name".into()),
        };

        match self.registry.heartbeat(&name) {
            Ok(()) => RegistryResponse::ok(),
            Err(e) => RegistryResponse::from_error(e),
        }
    }

    /// Traite List
    fn handle_list(&self) -> RegistryResponse {
        let services = self.registry.list();
        RegistryResponse::list(services)
    }

    /// Traite ListByStatus
    fn handle_list_by_status(&self, request: RegistryRequest) -> RegistryResponse {
        let status = match request.status {
            Some(s) => s,
            None => return RegistryResponse::error("missing status".into()),
        };

        let services = self.registry.list_by_status(status);
        RegistryResponse::list(services)
    }

    /// Traite GetStats
    fn handle_get_stats(&self) -> RegistryResponse {
        let stats = self.registry.stats();

        let data = RegistryStatsData {
            total_lookups: stats.total_lookups.load(Ordering::Relaxed),
            cache_hits: stats.cache_hits.load(Ordering::Relaxed),
            cache_misses: stats.cache_misses.load(Ordering::Relaxed),
            bloom_rejections: stats.bloom_rejections.load(Ordering::Relaxed),
            total_registrations: stats.total_registrations.load(Ordering::Relaxed),
            total_unregistrations: stats.total_unregistrations.load(Ordering::Relaxed),
            active_services: stats.active_services.load(Ordering::Relaxed),
        };

        RegistryResponse::stats(data)
    }

    /// Retourne le nombre de requêtes traitées
    pub fn requests_processed(&self) -> u64 {
        self.requests_processed
    }

    /// Retourne une référence au registry
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Retourne une référence mutable au registry
    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    /// Retourne la configuration
    pub fn config(&self) -> &DaemonConfig {
        &self.config
    }

    /// Flush le registry (persistence)
    pub fn flush(&mut self) -> RegistryResult<()> {
        self.registry.flush()
    }

    /// Charge le registry depuis le storage
    pub fn load(&mut self) -> RegistryResult<()> {
        self.registry.load()
    }
}

impl Default for RegistryDaemon {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ServiceName, ServiceInfo, ServiceStatus};
    use crate::protocol::ResponseType;

    #[test]
    fn test_daemon_creation() {
        let daemon = RegistryDaemon::new();
        assert_eq!(daemon.requests_processed(), 0);
    }

    #[test]
    fn test_daemon_handle_ping() {
        let mut daemon = RegistryDaemon::new();
        let request = RegistryRequest::ping();
        let response = daemon.handle_request(request);

        assert_eq!(response.response_type, ResponseType::Pong);
        assert_eq!(daemon.requests_processed(), 1);
    }

    #[test]
    fn test_daemon_handle_register() {
        let mut daemon = RegistryDaemon::new();

        let name = ServiceName::new("test_service").unwrap();
        let info = ServiceInfo::new("/tmp/test.sock");
        let request = RegistryRequest::register(name, info);

        let response = daemon.handle_request(request);
        assert_eq!(response.response_type, ResponseType::Ok);
    }

    #[test]
    fn test_daemon_handle_lookup() {
        let mut daemon = RegistryDaemon::new();

        // Register first
        let name = ServiceName::new("test_service").unwrap();
        let info = ServiceInfo::new("/tmp/test.sock");
        daemon.registry_mut().register(name.clone(), info).unwrap();

        // Lookup
        let request = RegistryRequest::lookup(name);
        let response = daemon.handle_request(request);

        assert_eq!(response.response_type, ResponseType::Found);
        assert!(response.service_info.is_some());
    }

    #[test]
    fn test_daemon_handle_lookup_not_found() {
        let mut daemon = RegistryDaemon::new();

        let name = ServiceName::new("nonexistent").unwrap();
        let request = RegistryRequest::lookup(name);
        let response = daemon.handle_request(request);

        assert_eq!(response.response_type, ResponseType::NotFound);
    }

    #[test]
    fn test_daemon_handle_list() {
        let mut daemon = RegistryDaemon::new();

        // Register some services
        for i in 0..3 {
            let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
            let info = ServiceInfo::new(&alloc::format!("/tmp/service_{}.sock", i));
            daemon.registry_mut().register(name, info).unwrap();
        }

        let request = RegistryRequest::list();
        let response = daemon.handle_request(request);

        assert_eq!(response.response_type, ResponseType::List);
        assert_eq!(response.services.len(), 3);
    }

    #[test]
    fn test_daemon_handle_unregister() {
        let mut daemon = RegistryDaemon::new();

        let name = ServiceName::new("test").unwrap();
        daemon
            .registry_mut()
            .register(name.clone(), ServiceInfo::new("/tmp/test.sock"))
            .unwrap();

        let request = RegistryRequest::unregister(name);
        let response = daemon.handle_request(request);

        assert_eq!(response.response_type, ResponseType::Ok);
    }

    #[test]
    fn test_daemon_handle_heartbeat() {
        let mut daemon = RegistryDaemon::new();

        let name = ServiceName::new("test").unwrap();
        daemon
            .registry_mut()
            .register(name.clone(), ServiceInfo::new("/tmp/test.sock"))
            .unwrap();

        let request = RegistryRequest::heartbeat(name);
        let response = daemon.handle_request(request);

        assert_eq!(response.response_type, ResponseType::Ok);
    }

    #[test]
    fn test_daemon_handle_get_stats() {
        let mut daemon = RegistryDaemon::new();

        let request = RegistryRequest::get_stats();
        let response = daemon.handle_request(request);

        assert_eq!(response.response_type, ResponseType::Stats);
        assert!(response.stats.is_some());
    }

    #[test]
    fn test_daemon_config() {
        let config = DaemonConfig::new()
            .with_max_connections(50)
            .with_queue_size(128)
            .with_verbose(true);

        assert_eq!(config.max_connections, 50);
        assert_eq!(config.request_queue_size, 128);
        assert!(config.verbose);
    }
}
