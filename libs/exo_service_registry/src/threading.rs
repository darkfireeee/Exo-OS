//! Thread-safe registry avec RwLock
//!
//! Implémentation multi-threaded du registry permettant
//! des lookups concurrents et des écritures thread-safe.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::{Registry, RegistryConfig, ServiceName, ServiceInfo, ServiceStatus, RegistryResult};

/// Registry thread-safe avec RwLock
///
/// Permet:
/// - Multiples readers concurrents
/// - Single writer à la fois
/// - Pas de deadlock
pub struct ThreadSafeRegistry {
    /// Registry sous-jacent protégé par RwLock
    inner: Arc<spin::RwLock<Registry>>,
}

impl ThreadSafeRegistry {
    /// Crée un nouveau registry thread-safe
    pub fn new() -> Self {
        Self {
            inner: Arc::new(spin::RwLock::new(Registry::new())),
        }
    }

    /// Crée avec configuration custom
    pub fn with_config(config: RegistryConfig) -> Self {
        Self {
            inner: Arc::new(spin::RwLock::new(Registry::with_config(config))),
        }
    }

    /// Clone le handle (partage le même registry)
    pub fn clone_handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Lookup thread-safe (lecture partagée)
    ///
    /// Note: Utilise un write lock car le cache LRU nécessite &mut self
    pub fn lookup(&self, name: &ServiceName) -> Option<ServiceInfo> {
        let mut registry = self.inner.write();
        registry.lookup(name)
    }

    /// Register thread-safe (écriture exclusive)
    pub fn register(&self, name: ServiceName, info: ServiceInfo) -> RegistryResult<()> {
        let mut registry = self.inner.write();
        registry.register(name, info)
    }

    /// Unregister thread-safe (écriture exclusive)
    pub fn unregister(&self, name: &ServiceName) -> RegistryResult<()> {
        let mut registry = self.inner.write();
        registry.unregister(name)
    }

    /// Heartbeat thread-safe (écriture exclusive)
    pub fn heartbeat(&self, name: &ServiceName) -> RegistryResult<()> {
        let mut registry = self.inner.write();
        registry.heartbeat(name)
    }

    /// List all services (lecture partagée)
    pub fn list(&self) -> Vec<(ServiceName, ServiceInfo)> {
        let registry = self.inner.read();
        registry.list()
    }

    /// List by status (lecture partagée)
    pub fn list_by_status(&self, status: ServiceStatus) -> Vec<(ServiceName, ServiceInfo)> {
        let registry = self.inner.read();
        registry.list_by_status(status)
    }

    /// Flush (écriture exclusive)
    pub fn flush(&self) -> RegistryResult<()> {
        let mut registry = self.inner.write();
        registry.flush()
    }

    /// Load (écriture exclusive)
    pub fn load(&self) -> RegistryResult<()> {
        let mut registry = self.inner.write();
        registry.load()
    }

    /// Stats (lecture partagée)
    pub fn stats(&self) -> crate::RegistryStats {
        let registry = self.inner.read();
        registry.stats().clone()
    }
}

impl Default for ThreadSafeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Implémentation Send + Sync pour cross-thread usage
unsafe impl Send for ThreadSafeRegistry {}
unsafe impl Sync for ThreadSafeRegistry {}

/// Pool de registries pour load balancing
pub struct RegistryPool {
    /// Registries dans le pool
    registries: Vec<ThreadSafeRegistry>,

    /// Index du prochain registry (round-robin)
    next_index: core::sync::atomic::AtomicUsize,
}

impl RegistryPool {
    /// Crée un nouveau pool
    pub fn new(size: usize, config: RegistryConfig) -> Self {
        let mut registries = Vec::with_capacity(size);

        for _ in 0..size {
            registries.push(ThreadSafeRegistry::with_config(config.clone()));
        }

        Self {
            registries,
            next_index: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Taille du pool
    pub fn size(&self) -> usize {
        self.registries.len()
    }

    /// Récupère le prochain registry (round-robin)
    pub fn next(&self) -> &ThreadSafeRegistry {
        let index = self.next_index.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        &self.registries[index % self.registries.len()]
    }

    /// Récupère un registry par hash du service name (consistent hashing)
    pub fn by_hash(&self, name: &ServiceName) -> &ThreadSafeRegistry {
        let hash = self.hash_name(name);
        &self.registries[hash % self.registries.len()]
    }

    /// Hash simple d'un service name
    fn hash_name(&self, name: &ServiceName) -> usize {
        let bytes = name.as_str().as_bytes();
        let mut hash: usize = 5381;

        for &byte in bytes {
            hash = ((hash << 5).wrapping_add(hash)).wrapping_add(byte as usize);
        }

        hash
    }

    /// Lookup avec load balancing
    pub fn lookup(&self, name: &ServiceName) -> Option<ServiceInfo> {
        // Utilise consistent hashing pour toujours aller au même registry
        let registry = self.by_hash(name);
        registry.lookup(name)
    }

    /// Register avec load balancing
    pub fn register(&self, name: ServiceName, info: ServiceInfo) -> RegistryResult<()> {
        let registry = self.by_hash(&name);
        registry.register(name, info)
    }

    /// Unregister avec load balancing
    pub fn unregister(&self, name: &ServiceName) -> RegistryResult<()> {
        let registry = self.by_hash(name);
        registry.unregister(name)
    }

    /// Heartbeat avec load balancing
    pub fn heartbeat(&self, name: &ServiceName) -> RegistryResult<()> {
        let registry = self.by_hash(name);
        registry.heartbeat(name)
    }

    /// List all services (agrégat tous les registries)
    pub fn list_all(&self) -> Vec<(ServiceName, ServiceInfo)> {
        let mut all_services = Vec::new();

        for registry in &self.registries {
            all_services.extend(registry.list());
        }

        all_services
    }

    /// Compte total des services
    pub fn total_services(&self) -> usize {
        self.registries
            .iter()
            .map(|r| {
                use core::sync::atomic::Ordering;
                r.stats().active_services.load(Ordering::Relaxed)
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thread_safe_registry() {
        let registry = ThreadSafeRegistry::new();

        let name = ServiceName::new("test_service").unwrap();
        let info = ServiceInfo::new("/tmp/test.sock");

        registry.register(name.clone(), info).unwrap();

        let found = registry.lookup(&name).unwrap();
        assert_eq!(found.endpoint(), "/tmp/test.sock");
    }

    #[test]
    fn test_registry_pool() {
        let pool = RegistryPool::new(4, RegistryConfig::new());

        assert_eq!(pool.size(), 4);

        // Register some services
        for i in 0..10 {
            let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
            let info = ServiceInfo::new(&alloc::format!("/tmp/service_{}.sock", i));
            pool.register(name, info).unwrap();
        }

        // Check total
        assert_eq!(pool.total_services(), 10);

        // Lookup
        let name = ServiceName::new("service_5").unwrap();
        let found = pool.lookup(&name).unwrap();
        assert_eq!(found.endpoint(), "/tmp/service_5.sock");
    }

    #[test]
    fn test_consistent_hashing() {
        let pool = RegistryPool::new(4, RegistryConfig::new());

        let name = ServiceName::new("my_service").unwrap();

        // Même nom devrait toujours aller au même registry
        let idx1 = pool.by_hash(&name) as *const _ as usize;
        let idx2 = pool.by_hash(&name) as *const _ as usize;

        assert_eq!(idx1, idx2);
    }
}
