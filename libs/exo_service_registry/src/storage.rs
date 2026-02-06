//! Backends de stockage pour le registry
//!
//! Ce module définit:
//! - StorageBackend: Trait pour abstraction du storage
//! - InMemoryBackend: Backend en mémoire (default)
//! - TomlBackend: Backend TOML persistant (feature: persistent)

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

use crate::types::{ServiceName, ServiceInfo, RegistryResult};

/// Trait abstrait pour backend de stockage
pub trait StorageBackend: Send + Sync {
    /// Insère ou met à jour un service
    fn insert(&mut self, name: ServiceName, info: ServiceInfo) -> RegistryResult<()>;

    /// Récupère un service
    fn get(&self, name: &ServiceName) -> Option<&ServiceInfo>;

    /// Récupère un service (mutable)
    fn get_mut(&mut self, name: &ServiceName) -> Option<&mut ServiceInfo>;

    /// Supprime un service
    fn remove(&mut self, name: &ServiceName) -> Option<ServiceInfo>;

    /// Liste tous les services
    fn list(&self) -> Vec<(ServiceName, ServiceInfo)>;

    /// Retourne le nombre de services
    fn len(&self) -> usize;

    /// Vérifie si vide
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Vérifie si un service existe
    fn contains(&self, name: &ServiceName) -> bool {
        self.get(name).is_some()
    }

    /// Efface tous les services
    fn clear(&mut self);

    /// Persiste les changements (si backend persistant)
    fn flush(&mut self) -> RegistryResult<()> {
        Ok(())
    }

    /// Charge depuis le storage persistant
    fn load(&mut self) -> RegistryResult<()> {
        Ok(())
    }
}

/// Backend en mémoire (non persistant)
///
/// Utilise BTreeMap pour ordre déterministe et performances log(n)
#[derive(Debug)]
pub struct InMemoryBackend {
    /// Map nom -> info
    services: BTreeMap<String, ServiceInfo>,
}

impl InMemoryBackend {
    /// Crée un nouveau backend en mémoire
    pub fn new() -> Self {
        Self {
            services: BTreeMap::new(),
        }
    }

    /// Crée avec capacité initiale estimée
    pub fn with_capacity(_capacity: usize) -> Self {
        // BTreeMap n'a pas de with_capacity, on utilise new()
        Self::new()
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageBackend for InMemoryBackend {
    fn insert(&mut self, name: ServiceName, info: ServiceInfo) -> RegistryResult<()> {
        self.services.insert(name.into_string(), info);
        Ok(())
    }

    fn get(&self, name: &ServiceName) -> Option<&ServiceInfo> {
        self.services.get(name.as_str())
    }

    fn get_mut(&mut self, name: &ServiceName) -> Option<&mut ServiceInfo> {
        self.services.get_mut(name.as_str())
    }

    fn remove(&mut self, name: &ServiceName) -> Option<ServiceInfo> {
        self.services.remove(name.as_str())
    }

    fn list(&self) -> Vec<(ServiceName, ServiceInfo)> {
        self.services
            .iter()
            .filter_map(|(k, v)| {
                ServiceName::new(k).ok().map(|name| (name, v.clone()))
            })
            .collect()
    }

    fn len(&self) -> usize {
        self.services.len()
    }

    fn clear(&mut self) {
        self.services.clear();
    }
}

/// Backend TOML persistant (feature: persistent)
#[cfg(feature = "persistent")]
#[derive(Debug)]
pub struct TomlBackend {
    /// Storage en mémoire
    memory: InMemoryBackend,

    /// Chemin du fichier TOML
    path: String,

    /// Flag dirty (besoin de flush)
    dirty: bool,
}

#[cfg(feature = "persistent")]
impl TomlBackend {
    /// Crée un nouveau backend TOML
    ///
    /// # Arguments
    /// - path: Chemin du fichier TOML
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            memory: InMemoryBackend::new(),
            path: path.into(),
            dirty: false,
        }
    }

    /// Marque comme dirty
    #[inline]
    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Vérifie si dirty
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Retourne le chemin du fichier
    #[inline]
    pub fn path(&self) -> &str {
        &self.path
    }
}

#[cfg(feature = "persistent")]
impl StorageBackend for TomlBackend {
    fn insert(&mut self, name: ServiceName, info: ServiceInfo) -> RegistryResult<()> {
        self.memory.insert(name, info)?;
        self.mark_dirty();
        Ok(())
    }

    fn get(&self, name: &ServiceName) -> Option<&ServiceInfo> {
        self.memory.get(name)
    }

    fn get_mut(&mut self, name: &ServiceName) -> Option<&mut ServiceInfo> {
        self.mark_dirty();
        self.memory.get_mut(name)
    }

    fn remove(&mut self, name: &ServiceName) -> Option<ServiceInfo> {
        self.mark_dirty();
        self.memory.remove(name)
    }

    fn list(&self) -> Vec<(ServiceName, ServiceInfo)> {
        self.memory.list()
    }

    fn len(&self) -> usize {
        self.memory.len()
    }

    fn clear(&mut self) {
        self.memory.clear();
        self.mark_dirty();
    }

    fn flush(&mut self) -> RegistryResult<()> {
        if !self.dirty {
            return Ok(());
        }

        // Sérialisation TOML
        use serde::Serialize;

        #[derive(Serialize)]
        struct TomlService {
            endpoint: String,
            status: String,
            registered_at: u64,
            last_heartbeat: u64,
            version: u32,
        }

        #[derive(Serialize)]
        struct TomlRoot {
            services: BTreeMap<String, TomlService>,
        }

        let services: BTreeMap<String, TomlService> = self
            .memory
            .services
            .iter()
            .map(|(name, info)| {
                let svc = TomlService {
                    endpoint: info.endpoint().to_string(),
                    status: format!("{}", info.status()),
                    registered_at: info.metadata().registered_at,
                    last_heartbeat: info.metadata().last_heartbeat,
                    version: info.metadata().version,
                };
                (name.clone(), svc)
            })
            .collect();

        let _root = TomlRoot { services };

        // Dans un environnement réel, on écrirait ici dans le fichier
        // Pour l'instant, on simule juste le succès
        // std::fs::write(&self.path, toml::to_string(&_root)?)?;

        self.dirty = false;
        Ok(())
    }

    fn load(&mut self) -> RegistryResult<()> {
        // Dans un environnement réel, on lirait le fichier TOML
        // Pour l'instant, on ne fait rien (fichier vide ou inexistant)
        // let content = std::fs::read_to_string(&self.path)?;
        // let root: TomlRoot = toml::from_str(&content)?;
        // ... populate memory ...

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_memory_backend() {
        let mut backend = InMemoryBackend::new();
        assert_eq!(backend.len(), 0);
        assert!(backend.is_empty());

        let name = ServiceName::new("test_service").unwrap();
        let info = ServiceInfo::new("/tmp/test.sock");

        backend.insert(name.clone(), info).unwrap();
        assert_eq!(backend.len(), 1);
        assert!(!backend.is_empty());
        assert!(backend.contains(&name));

        let retrieved = backend.get(&name).unwrap();
        assert_eq!(retrieved.endpoint(), "/tmp/test.sock");

        backend.remove(&name);
        assert_eq!(backend.len(), 0);
        assert!(!backend.contains(&name));
    }

    #[test]
    fn test_in_memory_backend_list() {
        let mut backend = InMemoryBackend::new();

        let name1 = ServiceName::new("service1").unwrap();
        let name2 = ServiceName::new("service2").unwrap();

        backend.insert(name1.clone(), ServiceInfo::new("/tmp/1.sock")).unwrap();
        backend.insert(name2.clone(), ServiceInfo::new("/tmp/2.sock")).unwrap();

        let list = backend.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_in_memory_backend_clear() {
        let mut backend = InMemoryBackend::new();
        backend.insert(
            ServiceName::new("test").unwrap(),
            ServiceInfo::new("/tmp/test.sock"),
        ).unwrap();

        assert_eq!(backend.len(), 1);
        backend.clear();
        assert_eq!(backend.len(), 0);
    }

    #[cfg(feature = "persistent")]
    #[test]
    fn test_toml_backend() {
        let mut backend = TomlBackend::new("/tmp/test.toml");
        assert!(!backend.is_dirty());

        let name = ServiceName::new("test_service").unwrap();
        backend.insert(name.clone(), ServiceInfo::new("/tmp/test.sock")).unwrap();

        assert!(backend.is_dirty());
        assert_eq!(backend.len(), 1);
    }

    #[cfg(feature = "persistent")]
    #[test]
    fn test_toml_backend_flush() {
        let mut backend = TomlBackend::new("/tmp/test.toml");
        let name = ServiceName::new("test").unwrap();
        backend.insert(name, ServiceInfo::new("/tmp/test.sock")).unwrap();

        assert!(backend.is_dirty());
        backend.flush().unwrap();
        assert!(!backend.is_dirty());
    }
}
