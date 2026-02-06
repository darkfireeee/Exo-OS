//! Client de discovery pour lookup de services
//!
//! Fournit une API client pour:
//! - Lookup synchrone avec retry
//! - Lookup asynchrone (future)
//! - Watch/Subscribe à des événements de service
//! - Connection pooling

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::types::{ServiceName, ServiceInfo, RegistryError, RegistryResult};

/// Client de discovery
///
/// Interface high-level pour lookup de services
pub struct DiscoveryClient {
    /// Retry count par défaut
    max_retries: u32,

    /// Timeout en millisecondes
    timeout_ms: u64,

    /// Cache local des lookups réussis
    cache: Vec<(String, ServiceInfo)>,
}

impl DiscoveryClient {
    /// Crée un nouveau client de discovery
    pub fn new() -> Self {
        Self {
            max_retries: 3,
            timeout_ms: 5000,
            cache: Vec::new(),
        }
    }

    /// Définit le nombre de retries
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Définit le timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Lookup un service par nom
    ///
    /// # Arguments
    /// - name: Nom du service
    ///
    /// # Returns
    /// - Ok(ServiceInfo) si trouvé
    /// - Err(ServiceNotFound) si non trouvé après retries
    pub fn find(&self, name: &ServiceName) -> RegistryResult<ServiceInfo> {
        // Dans une vraie implémentation, on ferait un IPC vers le registry
        // Pour l'instant, on retourne une erreur
        Err(RegistryError::ServiceNotFound(name.to_string()))
    }

    /// Lookup avec retry
    pub fn find_with_retry(&self, name: &ServiceName) -> RegistryResult<ServiceInfo> {
        let mut attempts = 0;

        loop {
            match self.find(name) {
                Ok(info) => return Ok(info),
                Err(e) => {
                    attempts += 1;
                    if attempts >= self.max_retries {
                        return Err(e);
                    }
                    // Dans une vraie implémentation, on attendrait un backoff exponentiel
                }
            }
        }
    }

    /// Vérifie si un service existe
    pub fn exists(&self, name: &ServiceName) -> bool {
        self.find(name).is_ok()
    }

    /// Liste tous les services disponibles
    pub fn list_available(&self) -> Vec<ServiceInfo> {
        // Dans une vraie implémentation, IPC vers registry.list()
        Vec::new()
    }

    /// Retourne le timeout configuré
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// Retourne le nombre max de retries
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }
}

impl Default for DiscoveryClient {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for DiscoveryClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiscoveryClient")
            .field("max_retries", &self.max_retries)
            .field("timeout_ms", &self.timeout_ms)
            .field("cache_size", &self.cache.len())
            .finish()
    }
}

/// Alias pour Discovery (API publique)
pub type Discovery = DiscoveryClient;

/// Service watcher pour notifications de changements
///
/// Permet de s'abonner aux événements:
/// - ServiceRegistered
/// - ServiceUnregistered
/// - ServiceStatusChanged
/// - ServiceFailed
#[derive(Debug)]
pub struct ServiceWatcher {
    /// Services surveillés
    watched: Vec<String>,

    /// Buffer d'événements
    events: Vec<WatchEvent>,
}

impl ServiceWatcher {
    /// Crée un nouveau watcher
    pub fn new() -> Self {
        Self {
            watched: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Ajoute un service à surveiller
    pub fn watch(&mut self, name: ServiceName) {
        self.watched.push(name.into_string());
    }

    /// Retire un service de la surveillance
    pub fn unwatch(&mut self, name: &ServiceName) {
        self.watched.retain(|s| s != name.as_str());
    }

    /// Poll les événements en attente
    pub fn poll_events(&mut self) -> Vec<WatchEvent> {
        core::mem::take(&mut self.events)
    }

    /// Vérifie si un service est surveillé
    pub fn is_watching(&self, name: &ServiceName) -> bool {
        self.watched.iter().any(|s| s == name.as_str())
    }
}

impl Default for ServiceWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Événement de watcher
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// Service enregistré
    Registered {
        /// Nom du service
        name: ServiceName,
        /// Info du service
        info: ServiceInfo,
    },

    /// Service désenregistré
    Unregistered {
        /// Nom du service
        name: ServiceName,
    },

    /// Statut changé
    StatusChanged {
        /// Nom du service
        name: ServiceName,
        /// Ancien statut
        old_status: crate::types::ServiceStatus,
        /// Nouveau statut
        new_status: crate::types::ServiceStatus,
    },

    /// Service en échec
    Failed {
        /// Nom du service
        name: ServiceName,
        /// Message d'erreur
        reason: String,
    },
}

impl fmt::Display for WatchEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registered { name, .. } => write!(f, "Service registered: {}", name),
            Self::Unregistered { name } => write!(f, "Service unregistered: {}", name),
            Self::StatusChanged { name, old_status, new_status } => {
                write!(f, "Service {} status: {} -> {}", name, old_status, new_status)
            }
            Self::Failed { name, reason } => write!(f, "Service {} failed: {}", name, reason),
        }
    }
}

/// Builder pour configuration de discovery avancée
#[derive(Debug, Clone)]
pub struct DiscoveryBuilder {
    max_retries: u32,
    timeout_ms: u64,
    enable_cache: bool,
    cache_size: usize,
}

impl DiscoveryBuilder {
    /// Crée un nouveau builder
    pub fn new() -> Self {
        Self {
            max_retries: 3,
            timeout_ms: 5000,
            enable_cache: true,
            cache_size: 50,
        }
    }

    /// Définit le nombre de retries
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Définit le timeout
    pub fn timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout_ms = timeout;
        self
    }

    /// Active/désactive le cache
    pub fn enable_cache(mut self, enable: bool) -> Self {
        self.enable_cache = enable;
        self
    }

    /// Définit la taille du cache
    pub fn cache_size(mut self, size: usize) -> Self {
        self.cache_size = size;
        self
    }

    /// Construit le client de discovery
    pub fn build(self) -> DiscoveryClient {
        DiscoveryClient::new()
            .with_max_retries(self.max_retries)
            .with_timeout(self.timeout_ms)
    }
}

impl Default for DiscoveryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_client_new() {
        let client = DiscoveryClient::new();
        assert_eq!(client.max_retries(), 3);
        assert_eq!(client.timeout_ms(), 5000);
    }

    #[test]
    fn test_discovery_client_builder() {
        let client = DiscoveryClient::new()
            .with_max_retries(5)
            .with_timeout(10000);

        assert_eq!(client.max_retries(), 5);
        assert_eq!(client.timeout_ms(), 10000);
    }

    #[test]
    fn test_service_watcher() {
        let mut watcher = ServiceWatcher::new();
        let name = ServiceName::new("test_service").unwrap();

        watcher.watch(name.clone());
        assert!(watcher.is_watching(&name));

        watcher.unwatch(&name);
        assert!(!watcher.is_watching(&name));
    }

    #[test]
    fn test_discovery_builder() {
        let builder = DiscoveryBuilder::new()
            .max_retries(10)
            .timeout_ms(3000)
            .cache_size(100);

        let client = builder.build();
        assert_eq!(client.max_retries(), 10);
        assert_eq!(client.timeout_ms(), 3000);
    }

    #[test]
    fn test_watch_event_display() {
        use crate::types::ServiceStatus;

        let name = ServiceName::new("test").unwrap();
        let event = WatchEvent::Registered {
            name: name.clone(),
            info: ServiceInfo::new("/tmp/test.sock"),
        };

        let display = format!("{}", event);
        assert!(display.contains("registered"));
        assert!(display.contains("test"));
    }
}
