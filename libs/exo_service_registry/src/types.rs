//! Types partagés pour le service registry
//!
//! Ce module définit les types fondamentaux:
//! - ServiceName: Nom validé de service
//! - ServiceInfo: Information complète sur un service
//! - ServiceMetadata: Métadonnées (timestamps, version)
//! - ServiceStatus: État du service
//! - RegistryError: Erreurs du registry

use alloc::string::String;
use core::fmt;

/// Nom de service validé
///
/// Format: `{category}_{name}` ou `{name}_service`
/// Longueur: 1-64 caractères
/// Caractères autorisés: a-z, 0-9, _, -
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceName(String);

impl ServiceName {
    /// Crée un nouveau nom de service avec validation
    ///
    /// # Validation
    /// - Longueur: 1-64 caractères
    /// - Caractères: [a-z0-9_-]
    /// - Pas de double underscore
    /// - Doit commencer par une lettre
    ///
    /// # Examples
    /// ```ignore
    /// let name = ServiceName::new("fs_service")?;
    /// let name = ServiceName::new("net_manager")?;
    /// ```
    pub fn new(name: &str) -> RegistryResult<Self> {
        // Vérification longueur
        if name.is_empty() {
            return Err(RegistryError::InvalidServiceName("empty name".into()));
        }
        if name.len() > crate::MAX_SERVICE_NAME_LEN {
            return Err(RegistryError::InvalidServiceName("name too long".into()));
        }

        // Doit commencer par une lettre
        if !name.chars().next().unwrap().is_ascii_lowercase() {
            return Err(RegistryError::InvalidServiceName(
                "must start with lowercase letter".into(),
            ));
        }

        // Validation caractères
        for ch in name.chars() {
            if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '_' && ch != '-' {
                return Err(RegistryError::InvalidServiceName(
                    "invalid character".into(),
                ));
            }
        }

        // Pas de double underscore
        if name.contains("__") {
            return Err(RegistryError::InvalidServiceName(
                "double underscore not allowed".into(),
            ));
        }

        Ok(Self(String::from(name)))
    }

    /// Crée un nom sans validation (unsafe)
    ///
    /// # Safety
    /// L'appelant doit garantir que le nom est valide
    pub const unsafe fn new_unchecked(name: String) -> Self {
        Self(name)
    }

    /// Retourne le nom comme &str
    #[inline(always)]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convertit en String (consomme)
    #[inline]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for ServiceName {
    #[inline(always)]
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServiceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Statut d'un service
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceStatus {
    /// Service en cours d'enregistrement
    Registering = 0,

    /// Service actif et disponible
    Active = 1,

    /// Service en pause (accepte pas de nouvelles requêtes)
    Paused = 2,

    /// Service dégradé (fonctionne mais performance réduite)
    Degraded = 3,

    /// Service en arrêt gracieux
    Stopping = 4,

    /// Service arrêté
    Stopped = 5,

    /// Service crashé
    Failed = 6,

    /// Service inconnu
    Unknown = 255,
}

impl ServiceStatus {
    /// Vérifie si le service est disponible
    #[inline]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Active | Self::Degraded)
    }

    /// Vérifie si le service est en erreur
    #[inline]
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::Failed)
    }

    /// Vérifie si le service est arrêté
    #[inline]
    pub const fn is_stopped(&self) -> bool {
        matches!(self, Self::Stopped | Self::Failed)
    }
}

impl Default for ServiceStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

impl fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registering => write!(f, "registering"),
            Self::Active => write!(f, "active"),
            Self::Paused => write!(f, "paused"),
            Self::Degraded => write!(f, "degraded"),
            Self::Stopping => write!(f, "stopping"),
            Self::Stopped => write!(f, "stopped"),
            Self::Failed => write!(f, "failed"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Métadonnées d'un service
#[derive(Debug, Clone)]
pub struct ServiceMetadata {
    /// Timestamp d'enregistrement (secondes depuis epoch)
    pub registered_at: u64,

    /// Timestamp dernier heartbeat (secondes depuis epoch)
    pub last_heartbeat: u64,

    /// Version du service
    pub version: u32,

    /// Nombre d'échecs consécutifs
    pub failure_count: u32,

    /// Flags de métadonnées
    pub flags: u32,
}

impl ServiceMetadata {
    /// Crée des métadonnées avec timestamp actuel
    pub fn new(timestamp: u64) -> Self {
        Self {
            registered_at: timestamp,
            last_heartbeat: timestamp,
            version: 1,
            failure_count: 0,
            flags: 0,
        }
    }

    /// Met à jour le heartbeat
    #[inline]
    pub fn update_heartbeat(&mut self, timestamp: u64) {
        self.last_heartbeat = timestamp;
        self.failure_count = 0;
    }

    /// Enregistre un échec
    #[inline]
    pub fn record_failure(&mut self) {
        self.failure_count = self.failure_count.saturating_add(1);
    }

    /// Vérifie si le service est stale (pas de heartbeat depuis N secondes)
    #[inline]
    pub fn is_stale(&self, current_time: u64, threshold_secs: u64) -> bool {
        current_time.saturating_sub(self.last_heartbeat) > threshold_secs
    }
}

impl Default for ServiceMetadata {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Information complète sur un service
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    /// Endpoint du service (socket path, etc.)
    endpoint: String,

    /// Statut actuel
    status: ServiceStatus,

    /// Métadonnées
    metadata: ServiceMetadata,
}

impl ServiceInfo {
    /// Crée une nouvelle info de service
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            status: ServiceStatus::Registering,
            metadata: ServiceMetadata::default(),
        }
    }

    /// Crée avec timestamp
    pub fn with_timestamp(endpoint: impl Into<String>, timestamp: u64) -> Self {
        Self {
            endpoint: endpoint.into(),
            status: ServiceStatus::Registering,
            metadata: ServiceMetadata::new(timestamp),
        }
    }

    /// Retourne l'endpoint
    #[inline]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Retourne le statut
    #[inline]
    pub fn status(&self) -> ServiceStatus {
        self.status
    }

    /// Retourne les métadonnées
    #[inline]
    pub fn metadata(&self) -> &ServiceMetadata {
        &self.metadata
    }

    /// Retourne les métadonnées mutables
    #[inline]
    pub fn metadata_mut(&mut self) -> &mut ServiceMetadata {
        &mut self.metadata
    }

    /// Définit le statut
    #[inline]
    pub fn set_status(&mut self, status: ServiceStatus) {
        self.status = status;
    }

    /// Vérifie si le service est disponible
    #[inline]
    pub fn is_available(&self) -> bool {
        self.status.is_available()
    }

    /// Vérifie si le service est stale
    #[inline]
    pub fn is_stale(&self, current_time: u64, threshold_secs: u64) -> bool {
        self.metadata.is_stale(current_time, threshold_secs)
    }

    /// Met à jour le heartbeat
    #[inline]
    pub fn update_heartbeat(&mut self, timestamp: u64) {
        self.metadata.update_heartbeat(timestamp);
        if self.status == ServiceStatus::Degraded || self.status == ServiceStatus::Failed {
            self.status = ServiceStatus::Active;
        }
    }

    /// Enregistre un échec
    #[inline]
    pub fn record_failure(&mut self) {
        self.metadata.record_failure();
        if self.metadata.failure_count >= 3 {
            self.status = ServiceStatus::Failed;
        } else if self.metadata.failure_count >= 1 {
            self.status = ServiceStatus::Degraded;
        }
    }

    /// Active le service
    #[inline]
    pub fn activate(&mut self) {
        self.status = ServiceStatus::Active;
    }
}

/// Erreurs du registry
#[derive(Debug, Clone)]
pub enum RegistryError {
    /// Nom de service invalide
    InvalidServiceName(String),

    /// Service non trouvé
    ServiceNotFound(String),

    /// Service déjà enregistré
    ServiceAlreadyExists(String),

    /// Endpoint invalide
    InvalidEndpoint(String),

    /// Erreur de storage
    StorageError(String),

    /// Erreur de cache
    CacheError(String),

    /// Erreur de health check
    HealthCheckError(String),

    /// Erreur de sérialisation
    SerializationError(String),

    /// Service non disponible
    ServiceUnavailable(String),

    /// Erreur interne
    InternalError(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidServiceName(msg) => write!(f, "invalid service name: {}", msg),
            Self::ServiceNotFound(name) => write!(f, "service not found: {}", name),
            Self::ServiceAlreadyExists(name) => write!(f, "service already exists: {}", name),
            Self::InvalidEndpoint(msg) => write!(f, "invalid endpoint: {}", msg),
            Self::StorageError(msg) => write!(f, "storage error: {}", msg),
            Self::CacheError(msg) => write!(f, "cache error: {}", msg),
            Self::HealthCheckError(msg) => write!(f, "health check error: {}", msg),
            Self::SerializationError(msg) => write!(f, "serialization error: {}", msg),
            Self::ServiceUnavailable(name) => write!(f, "service unavailable: {}", name),
            Self::InternalError(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

/// Type Result pour le registry
pub type RegistryResult<T> = Result<T, RegistryError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_name_valid() {
        assert!(ServiceName::new("fs_service").is_ok());
        assert!(ServiceName::new("net_manager").is_ok());
        assert!(ServiceName::new("logger-daemon").is_ok());
        assert!(ServiceName::new("a").is_ok());
    }

    #[test]
    fn test_service_name_invalid() {
        assert!(ServiceName::new("").is_err());
        assert!(ServiceName::new("FS_SERVICE").is_err()); // uppercase
        assert!(ServiceName::new("9service").is_err()); // start with digit
        assert!(ServiceName::new("service__bad").is_err()); // double underscore
        assert!(ServiceName::new("service.name").is_err()); // invalid char
    }

    #[test]
    fn test_service_status() {
        assert!(ServiceStatus::Active.is_available());
        assert!(ServiceStatus::Degraded.is_available());
        assert!(!ServiceStatus::Paused.is_available());
        assert!(!ServiceStatus::Stopped.is_available());

        assert!(ServiceStatus::Failed.is_error());
        assert!(!ServiceStatus::Active.is_error());

        assert!(ServiceStatus::Stopped.is_stopped());
        assert!(ServiceStatus::Failed.is_stopped());
        assert!(!ServiceStatus::Active.is_stopped());
    }

    #[test]
    fn test_service_metadata() {
        let mut meta = ServiceMetadata::new(1000);
        assert_eq!(meta.registered_at, 1000);
        assert_eq!(meta.last_heartbeat, 1000);
        assert_eq!(meta.failure_count, 0);

        meta.update_heartbeat(2000);
        assert_eq!(meta.last_heartbeat, 2000);
        assert_eq!(meta.failure_count, 0);

        meta.record_failure();
        assert_eq!(meta.failure_count, 1);

        assert!(!meta.is_stale(2050, 60));
        assert!(meta.is_stale(2100, 60));
    }

    #[test]
    fn test_service_info() {
        let mut info = ServiceInfo::with_timestamp("/tmp/service.sock", 1000);
        assert_eq!(info.endpoint(), "/tmp/service.sock");
        assert_eq!(info.status(), ServiceStatus::Registering);

        info.activate();
        assert_eq!(info.status(), ServiceStatus::Active);
        assert!(info.is_available());

        info.record_failure();
        assert_eq!(info.status(), ServiceStatus::Degraded);
        assert_eq!(info.metadata().failure_count, 1);

        info.record_failure();
        info.record_failure();
        assert_eq!(info.status(), ServiceStatus::Failed);
        assert!(!info.is_available());

        info.update_heartbeat(2000);
        assert_eq!(info.status(), ServiceStatus::Active);
        assert_eq!(info.metadata().failure_count, 0);
    }

    #[test]
    fn test_service_name_display() {
        let name = ServiceName::new("test_service").unwrap();
        assert_eq!(format!("{}", name), "test_service");
    }

    #[test]
    fn test_service_status_display() {
        assert_eq!(format!("{}", ServiceStatus::Active), "active");
        assert_eq!(format!("{}", ServiceStatus::Failed), "failed");
    }

    #[test]
    fn test_error_display() {
        let err = RegistryError::ServiceNotFound("test".into());
        assert_eq!(format!("{}", err), "service not found: test");
    }
}
