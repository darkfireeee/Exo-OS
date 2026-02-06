//! Protocole IPC pour service registry
//!
//! Définit les messages et le protocole de communication entre:
//! - Registry daemon (serveur)
//! - Discovery clients (clients)
//!
//! Architecture:
//! ```text
//! Client → [Request] → Daemon → [Process] → [Response] → Client
//! ```

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::types::{ServiceName, ServiceInfo, ServiceStatus, RegistryError, RegistryResult};

/// Version du protocole registry
pub const REGISTRY_PROTOCOL_VERSION: u16 = 1;

/// Type de requête registry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RequestType {
    /// Enregistrer un service
    Register = 1,

    /// Chercher un service
    Lookup = 2,

    /// Désenregistrer un service
    Unregister = 3,

    /// Heartbeat d'un service
    Heartbeat = 4,

    /// Lister tous les services
    List = 5,

    /// Lister par statut
    ListByStatus = 6,

    /// Obtenir les statistiques
    GetStats = 7,

    /// Ping (health check)
    Ping = 8,
}

impl RequestType {
    /// Convertit depuis u16
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::Register),
            2 => Some(Self::Lookup),
            3 => Some(Self::Unregister),
            4 => Some(Self::Heartbeat),
            5 => Some(Self::List),
            6 => Some(Self::ListByStatus),
            7 => Some(Self::GetStats),
            8 => Some(Self::Ping),
            _ => None,
        }
    }

    /// Convertit vers u16
    pub const fn as_u16(self) -> u16 {
        self as u16
    }
}

/// Type de réponse registry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ResponseType {
    /// Succès
    Ok = 0,

    /// Service trouvé (avec info)
    Found = 1,

    /// Service non trouvé
    NotFound = 2,

    /// Liste de services
    List = 3,

    /// Statistiques
    Stats = 4,

    /// Pong (réponse à Ping)
    Pong = 5,

    /// Erreur
    Error = 255,
}

impl ResponseType {
    /// Convertit depuis u16
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::Ok),
            1 => Some(Self::Found),
            2 => Some(Self::NotFound),
            3 => Some(Self::List),
            4 => Some(Self::Stats),
            5 => Some(Self::Pong),
            255 => Some(Self::Error),
            _ => None,
        }
    }

    /// Convertit vers u16
    pub const fn as_u16(self) -> u16 {
        self as u16
    }
}

/// Requête registry (sérialisée)
///
/// Format compact pour IPC (message inline si possible)
#[derive(Debug, Clone)]
pub struct RegistryRequest {
    /// Type de requête
    pub request_type: RequestType,

    /// Nom du service (pour Register, Lookup, Unregister, Heartbeat)
    pub service_name: Option<ServiceName>,

    /// Info du service (pour Register)
    pub service_info: Option<ServiceInfo>,

    /// Statut (pour ListByStatus)
    pub status: Option<ServiceStatus>,
}

impl RegistryRequest {
    /// Crée une requête Register
    pub fn register(name: ServiceName, info: ServiceInfo) -> Self {
        Self {
            request_type: RequestType::Register,
            service_name: Some(name),
            service_info: Some(info),
            status: None,
        }
    }

    /// Crée une requête Lookup
    pub fn lookup(name: ServiceName) -> Self {
        Self {
            request_type: RequestType::Lookup,
            service_name: Some(name),
            service_info: None,
            status: None,
        }
    }

    /// Crée une requête Unregister
    pub fn unregister(name: ServiceName) -> Self {
        Self {
            request_type: RequestType::Unregister,
            service_name: Some(name),
            service_info: None,
            status: None,
        }
    }

    /// Crée une requête Heartbeat
    pub fn heartbeat(name: ServiceName) -> Self {
        Self {
            request_type: RequestType::Heartbeat,
            service_name: Some(name),
            service_info: None,
            status: None,
        }
    }

    /// Crée une requête List
    pub fn list() -> Self {
        Self {
            request_type: RequestType::List,
            service_name: None,
            service_info: None,
            status: None,
        }
    }

    /// Crée une requête ListByStatus
    pub fn list_by_status(status: ServiceStatus) -> Self {
        Self {
            request_type: RequestType::ListByStatus,
            service_name: None,
            service_info: None,
            status: Some(status),
        }
    }

    /// Crée une requête GetStats
    pub fn get_stats() -> Self {
        Self {
            request_type: RequestType::GetStats,
            service_name: None,
            service_info: None,
            status: None,
        }
    }

    /// Crée une requête Ping
    pub fn ping() -> Self {
        Self {
            request_type: RequestType::Ping,
            service_name: None,
            service_info: None,
            status: None,
        }
    }
}

/// Réponse registry (sérialisée)
#[derive(Debug, Clone)]
pub struct RegistryResponse {
    /// Type de réponse
    pub response_type: ResponseType,

    /// Info de service (pour Found)
    pub service_info: Option<ServiceInfo>,

    /// Liste de services (pour List)
    pub services: Vec<(ServiceName, ServiceInfo)>,

    /// Message d'erreur (pour Error)
    pub error_message: Option<String>,

    /// Statistiques (pour Stats)
    pub stats: Option<RegistryStatsData>,
}

impl RegistryResponse {
    /// Crée une réponse Ok
    pub fn ok() -> Self {
        Self {
            response_type: ResponseType::Ok,
            service_info: None,
            services: Vec::new(),
            error_message: None,
            stats: None,
        }
    }

    /// Crée une réponse Found
    pub fn found(info: ServiceInfo) -> Self {
        Self {
            response_type: ResponseType::Found,
            service_info: Some(info),
            services: Vec::new(),
            error_message: None,
            stats: None,
        }
    }

    /// Crée une réponse NotFound
    pub fn not_found() -> Self {
        Self {
            response_type: ResponseType::NotFound,
            service_info: None,
            services: Vec::new(),
            error_message: None,
            stats: None,
        }
    }

    /// Crée une réponse List
    pub fn list(services: Vec<(ServiceName, ServiceInfo)>) -> Self {
        Self {
            response_type: ResponseType::List,
            service_info: None,
            services,
            error_message: None,
            stats: None,
        }
    }

    /// Crée une réponse Stats
    pub fn stats(stats: RegistryStatsData) -> Self {
        Self {
            response_type: ResponseType::Stats,
            service_info: None,
            services: Vec::new(),
            error_message: None,
            stats: Some(stats),
        }
    }

    /// Crée une réponse Pong
    pub fn pong() -> Self {
        Self {
            response_type: ResponseType::Pong,
            service_info: None,
            services: Vec::new(),
            error_message: None,
            stats: None,
        }
    }

    /// Crée une réponse Error
    pub fn error(message: String) -> Self {
        Self {
            response_type: ResponseType::Error,
            service_info: None,
            services: Vec::new(),
            error_message: Some(message),
            stats: None,
        }
    }

    /// Convertit depuis RegistryError
    pub fn from_error(error: RegistryError) -> Self {
        Self::error(alloc::format!("{}", error))
    }
}

/// Statistiques du registry (sérialisable)
#[derive(Debug, Clone, Copy)]
pub struct RegistryStatsData {
    /// Nombre total de lookups
    pub total_lookups: u64,

    /// Nombre de cache hits
    pub cache_hits: u64,

    /// Nombre de cache misses
    pub cache_misses: u64,

    /// Nombre de bloom rejections
    pub bloom_rejections: u64,

    /// Nombre total de registrations
    pub total_registrations: u64,

    /// Nombre total de unregistrations
    pub total_unregistrations: u64,

    /// Nombre de services actifs
    pub active_services: usize,
}

impl RegistryStatsData {
    /// Taux de cache hit
    pub fn cache_hit_rate(&self) -> f64 {
        if self.total_lookups == 0 {
            0.0
        } else {
            self.cache_hits as f64 / self.total_lookups as f64
        }
    }

    /// Taux de bloom filter rejection
    pub fn bloom_rejection_rate(&self) -> f64 {
        if self.total_lookups == 0 {
            0.0
        } else {
            self.bloom_rejections as f64 / self.total_lookups as f64
        }
    }
}

impl fmt::Display for RegistryStatsData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Stats: lookups={}, hits={} ({:.1}%), active={}",
            self.total_lookups,
            self.cache_hits,
            self.cache_hit_rate() * 100.0,
            self.active_services
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_type_conversion() {
        assert_eq!(RequestType::from_u16(1), Some(RequestType::Register));
        assert_eq!(RequestType::from_u16(2), Some(RequestType::Lookup));
        assert_eq!(RequestType::from_u16(99), None);

        assert_eq!(RequestType::Register.as_u16(), 1);
        assert_eq!(RequestType::Lookup.as_u16(), 2);
    }

    #[test]
    fn test_response_type_conversion() {
        assert_eq!(ResponseType::from_u16(0), Some(ResponseType::Ok));
        assert_eq!(ResponseType::from_u16(1), Some(ResponseType::Found));
        assert_eq!(ResponseType::from_u16(255), Some(ResponseType::Error));
        assert_eq!(ResponseType::from_u16(100), None);
    }

    #[test]
    fn test_registry_request_builders() {
        let name = ServiceName::new("test").unwrap();
        let info = ServiceInfo::new("/tmp/test.sock");

        let req = RegistryRequest::register(name.clone(), info);
        assert_eq!(req.request_type, RequestType::Register);
        assert!(req.service_name.is_some());
        assert!(req.service_info.is_some());

        let req = RegistryRequest::lookup(name.clone());
        assert_eq!(req.request_type, RequestType::Lookup);
        assert!(req.service_name.is_some());
        assert!(req.service_info.is_none());

        let req = RegistryRequest::ping();
        assert_eq!(req.request_type, RequestType::Ping);
    }

    #[test]
    fn test_registry_response_builders() {
        let resp = RegistryResponse::ok();
        assert_eq!(resp.response_type, ResponseType::Ok);

        let resp = RegistryResponse::not_found();
        assert_eq!(resp.response_type, ResponseType::NotFound);

        let resp = RegistryResponse::error("test error".into());
        assert_eq!(resp.response_type, ResponseType::Error);
        assert_eq!(resp.error_message, Some("test error".into()));

        let resp = RegistryResponse::pong();
        assert_eq!(resp.response_type, ResponseType::Pong);
    }

    #[test]
    fn test_stats_data() {
        let stats = RegistryStatsData {
            total_lookups: 100,
            cache_hits: 90,
            cache_misses: 10,
            bloom_rejections: 5,
            total_registrations: 50,
            total_unregistrations: 5,
            active_services: 45,
        };

        assert_eq!(stats.cache_hit_rate(), 0.9);
        assert_eq!(stats.bloom_rejection_rate(), 0.05);

        let display = alloc::format!("{}", stats);
        assert!(display.contains("lookups=100"));
        assert!(display.contains("hits=90"));
    }
}
