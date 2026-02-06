//! Service versioning pour migrations et compatibility
//!
//! Permet de gérer plusieurs versions d'un même service et de coordonner
//! les migrations en douceur.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::types::{ServiceName, ServiceInfo, RegistryResult, RegistryError};

/// Version d'un service (semantic versioning)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ServiceVersion {
    /// Version majeure
    pub major: u16,
    /// Version mineure
    pub minor: u16,
    /// Version patch
    pub patch: u16,
}

impl ServiceVersion {
    /// Crée une nouvelle version
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self { major, minor, patch }
    }

    /// Parse depuis une string "major.minor.patch"
    pub fn parse(s: &str) -> Result<Self, &'static str> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err("Invalid version format");
        }

        let major = parts[0].parse().map_err(|_| "Invalid major version")?;
        let minor = parts[1].parse().map_err(|_| "Invalid minor version")?;
        let patch = parts[2].parse().map_err(|_| "Invalid patch version")?;

        Ok(Self::new(major, minor, patch))
    }

    /// Vérifie la compatibilité avec une autre version
    pub fn is_compatible_with(&self, other: &ServiceVersion) -> bool {
        // Compatible si même version majeure
        self.major == other.major && self >= other
    }

    /// Retourne true si c'est un breaking change
    pub fn is_breaking_change(&self, other: &ServiceVersion) -> bool {
        self.major != other.major
    }
}

impl fmt::Display for ServiceVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Service versionné
#[derive(Debug, Clone)]
pub struct VersionedService {
    /// Nom du service
    pub name: ServiceName,
    /// Version du service
    pub version: ServiceVersion,
    /// Informations du service
    pub info: ServiceInfo,
    /// Indique si la version est dépréciée
    pub deprecated: bool,
}

impl VersionedService {
    /// Crée un nouveau service versionné
    pub fn new(name: ServiceName, version: ServiceVersion, info: ServiceInfo) -> Self {
        Self {
            name,
            version,
            info,
            deprecated: false,
        }
    }

    /// Marque comme déprécié
    pub fn deprecate(&mut self) {
        self.deprecated = true;
    }

    /// Vérifie si compatible avec une version requise
    pub fn satisfies(&self, required: &ServiceVersion) -> bool {
        self.version.is_compatible_with(required)
    }
}

/// Gestionnaire de versions
pub struct VersionManager {
    /// Services versionnés
    services: Vec<VersionedService>,
}

impl VersionManager {
    /// Crée un nouveau gestionnaire
    pub fn new() -> Self {
        Self {
            services: Vec::new(),
        }
    }

    /// Enregistre un service versionné
    pub fn register(&mut self, service: VersionedService) -> RegistryResult<()> {
        // Vérifie qu'il n'existe pas déjà avec cette version
        if self.services.iter().any(|s| {
            s.name == service.name && s.version == service.version
        }) {
            return Err(RegistryError::ServiceAlreadyExists(
                service.name.as_str().into()
            ));
        }

        self.services.push(service);
        Ok(())
    }

    /// Trouve la meilleure version compatible
    pub fn find_compatible(
        &self,
        name: &ServiceName,
        required: &ServiceVersion,
    ) -> Option<&VersionedService> {
        self.services
            .iter()
            .filter(|s| s.name == *name && !s.deprecated)
            .filter(|s| s.satisfies(required))
            .max_by_key(|s| s.version)
    }

    /// Liste toutes les versions d'un service
    pub fn list_versions(&self, name: &ServiceName) -> Vec<&VersionedService> {
        self.services
            .iter()
            .filter(|s| s.name == *name)
            .collect()
    }

    /// Marque une version comme dépréciée
    pub fn deprecate_version(
        &mut self,
        name: &ServiceName,
        version: &ServiceVersion,
    ) -> RegistryResult<()> {
        let service = self.services
            .iter_mut()
            .find(|s| s.name == *name && s.version == *version)
            .ok_or_else(|| RegistryError::ServiceNotFound(name.as_str().into()))?;

        service.deprecate();
        Ok(())
    }

    /// Compte le nombre de versions actives
    pub fn count_active_versions(&self, name: &ServiceName) -> usize {
        self.services
            .iter()
            .filter(|s| s.name == *name && !s.deprecated)
            .count()
    }
}

impl Default for VersionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() {
        let v = ServiceVersion::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);

        assert!(ServiceVersion::parse("1.2").is_err());
        assert!(ServiceVersion::parse("abc").is_err());
    }

    #[test]
    fn test_version_compatibility() {
        let v1 = ServiceVersion::new(1, 0, 0);
        let v2 = ServiceVersion::new(1, 1, 0);
        let v3 = ServiceVersion::new(2, 0, 0);

        assert!(v2.is_compatible_with(&v1));
        assert!(!v1.is_compatible_with(&v2));
        assert!(!v3.is_compatible_with(&v1));
        assert!(v3.is_breaking_change(&v1));
    }

    #[test]
    fn test_version_manager() {
        let mut manager = VersionManager::new();

        let name = ServiceName::new("test_service").unwrap();
        let v1 = ServiceVersion::new(1, 0, 0);
        let v2 = ServiceVersion::new(1, 1, 0);

        let service1 = VersionedService::new(
            name.clone(),
            v1,
            ServiceInfo::new("/tmp/v1.sock"),
        );

        let service2 = VersionedService::new(
            name.clone(),
            v2,
            ServiceInfo::new("/tmp/v2.sock"),
        );

        manager.register(service1).unwrap();
        manager.register(service2).unwrap();

        // Trouve la meilleure version compatible avec 1.0.0
        let required = ServiceVersion::new(1, 0, 0);
        let found = manager.find_compatible(&name, &required).unwrap();
        assert_eq!(found.version, v2); // v2 est meilleur que v1

        // Liste toutes les versions
        let versions = manager.list_versions(&name);
        assert_eq!(versions.len(), 2);

        // Déprécier une version
        manager.deprecate_version(&name, &v1).unwrap();
        assert_eq!(manager.count_active_versions(&name), 1);
    }

    #[test]
    fn test_version_display() {
        let v = ServiceVersion::new(1, 2, 3);
        assert_eq!(alloc::format!("{}", v), "1.2.3");
    }
}
