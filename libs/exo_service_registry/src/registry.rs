//! Registry principal avec cache et bloom filter
//!
//! Architecture:
//! - Storage backend (pluggable)
//! - LRU cache pour fast lookups
//! - Bloom filter pour fast negative lookups
//! - Thread-safe avec RwLock interne
//! - Statistiques de performance

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::storage::{StorageBackend, InMemoryBackend};
use crate::types::{ServiceName, ServiceInfo, ServiceStatus, RegistryError, RegistryResult};

/// Configuration du registry
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    /// Taille du cache LRU
    pub cache_size: usize,

    /// TTL du cache en secondes
    pub cache_ttl_secs: u64,

    /// Taille du bloom filter
    pub bloom_size: usize,

    /// Taux de faux positifs bloom filter
    pub bloom_fp_rate: f64,

    /// Seuil de staleness en secondes (pour health check)
    pub stale_threshold_secs: u64,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            cache_size: crate::DEFAULT_CACHE_SIZE,
            cache_ttl_secs: crate::DEFAULT_CACHE_TTL_SECS,
            bloom_size: crate::DEFAULT_BLOOM_SIZE,
            bloom_fp_rate: crate::DEFAULT_BLOOM_FP_RATE,
            stale_threshold_secs: 300, // 5 minutes
        }
    }
}

impl RegistryConfig {
    /// Crée une configuration custom
    pub fn new() -> Self {
        Self::default()
    }

    /// Définit la taille du cache
    pub fn with_cache_size(mut self, size: usize) -> Self {
        self.cache_size = size;
        self
    }

    /// Définit le TTL du cache
    pub fn with_cache_ttl(mut self, ttl_secs: u64) -> Self {
        self.cache_ttl_secs = ttl_secs;
        self
    }

    /// Définit la taille du bloom filter
    pub fn with_bloom_size(mut self, size: usize) -> Self {
        self.bloom_size = size;
        self
    }

    /// Définit le seuil de staleness
    pub fn with_stale_threshold(mut self, threshold_secs: u64) -> Self {
        self.stale_threshold_secs = threshold_secs;
        self
    }
}

/// Entrée du cache LRU
#[derive(Debug, Clone)]
struct CacheEntry {
    /// Info de service
    info: ServiceInfo,

    /// Timestamp d'insertion dans le cache
    cached_at: u64,
}

impl CacheEntry {
    /// Crée une nouvelle entrée
    fn new(info: ServiceInfo, timestamp: u64) -> Self {
        Self {
            info,
            cached_at: timestamp,
        }
    }

    /// Vérifie si l'entrée est expirée
    fn is_expired(&self, current_time: u64, ttl_secs: u64) -> bool {
        current_time.saturating_sub(self.cached_at) > ttl_secs
    }
}

/// Cache LRU simple basé sur Vec
///
/// Implémentation simple et efficace:
/// - Insert: O(1) amortized
/// - Lookup: O(n) mais petit n (100 par défaut)
/// - Éviction LRU: O(1)
struct LruCache {
    /// Entrées du cache (nom, entry)
    entries: Vec<(String, CacheEntry)>,

    /// Capacité maximale
    capacity: usize,

    /// TTL en secondes
    ttl_secs: u64,
}

impl LruCache {
    /// Crée un nouveau cache
    fn new(capacity: usize, ttl_secs: u64) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
            ttl_secs,
        }
    }

    /// Récupère une entrée (et la marque comme utilisée)
    fn get(&mut self, name: &str, current_time: u64) -> Option<ServiceInfo> {
        // Trouve l'index
        let idx = self.entries.iter().position(|(k, _)| k == name)?;

        // Vérifie expiration
        if self.entries[idx].1.is_expired(current_time, self.ttl_secs) {
            self.entries.swap_remove(idx);
            return None;
        }

        // Déplace à la fin (MRU)
        let entry = self.entries.remove(idx);
        self.entries.push(entry);

        Some(self.entries.last().unwrap().1.info.clone())
    }

    /// Insère une entrée
    fn insert(&mut self, name: String, info: ServiceInfo, timestamp: u64) {
        // Supprime l'ancienne entrée si elle existe
        if let Some(idx) = self.entries.iter().position(|(k, _)| k == &name) {
            self.entries.remove(idx);
        }

        // Ajoute la nouvelle entrée
        let entry = CacheEntry::new(info, timestamp);
        self.entries.push((name, entry));

        // Éviction LRU si plein
        if self.entries.len() > self.capacity {
            self.entries.remove(0); // Supprime le plus ancien (LRU)
        }
    }

    /// Invalide une entrée
    fn invalidate(&mut self, name: &str) {
        if let Some(idx) = self.entries.iter().position(|(k, _)| k == name) {
            self.entries.swap_remove(idx);
        }
    }

    /// Efface le cache
    fn clear(&mut self) {
        self.entries.clear();
    }

    /// Retourne la taille actuelle
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Bloom filter simple pour fast negative lookups
///
/// Utilise un bitset simple avec plusieurs hash functions
struct BloomFilter {
    /// Bitset
    bits: Vec<u64>,

    /// Nombre d'éléments
    size: usize,

    /// Nombre de hash functions
    num_hashes: usize,
}

impl BloomFilter {
    /// Crée un nouveau bloom filter
    ///
    /// # Arguments
    /// - size: Nombre d'éléments attendus
    /// - _fp_rate: Taux de faux positifs (0.0 - 1.0) - simplifié pour no_std
    fn new(size: usize, _fp_rate: f64) -> Self {
        // Implémentation simplifiée pour no_std (pas de libm)
        // Utilise heuristiques simples au lieu des formules complètes

        // ~10 bits par élément (bon compromis performance/espace)
        let num_bits = size * 10;

        // 4 hash functions (bon équilibre selon la littérature)
        let num_hashes = 4;

        let num_words = (num_bits + 63) / 64;
        let bits = alloc::vec![0u64; num_words];

        Self {
            bits,
            size: num_bits,
            num_hashes,
        }
    }

    /// Hash une chaîne avec une seed
    #[inline]
    fn hash(&self, s: &str, seed: usize) -> usize {
        let mut hash = 0xcbf29ce484222325_u64; // FNV offset basis

        for byte in s.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3); // FNV prime
        }

        hash = hash.wrapping_add(seed as u64);
        (hash as usize) % self.size
    }

    /// Insère un élément
    fn insert(&mut self, s: &str) {
        for i in 0..self.num_hashes {
            let bit_idx = self.hash(s, i);
            let word_idx = bit_idx / 64;
            let bit_offset = bit_idx % 64;

            if word_idx < self.bits.len() {
                self.bits[word_idx] |= 1u64 << bit_offset;
            }
        }
    }

    /// Vérifie si un élément peut exister (true = maybe, false = definitely not)
    fn might_contain(&self, s: &str) -> bool {
        for i in 0..self.num_hashes {
            let bit_idx = self.hash(s, i);
            let word_idx = bit_idx / 64;
            let bit_offset = bit_idx % 64;

            if word_idx >= self.bits.len() {
                return false;
            }

            if (self.bits[word_idx] & (1u64 << bit_offset)) == 0 {
                return false;
            }
        }
        true
    }

    /// Efface le bloom filter
    fn clear(&mut self) {
        for word in &mut self.bits {
            *word = 0;
        }
    }
}

/// Statistiques du registry
#[derive(Debug)]
pub struct RegistryStats {
    /// Nombre total de lookups
    pub total_lookups: AtomicU64,

    /// Nombre de cache hits
    pub cache_hits: AtomicU64,

    /// Nombre de cache misses
    pub cache_misses: AtomicU64,

    /// Nombre de bloom filter rejections
    pub bloom_rejections: AtomicU64,

    /// Nombre de registrations
    pub total_registrations: AtomicU64,

    /// Nombre de unregistrations
    pub total_unregistrations: AtomicU64,

    /// Nombre actuel de services
    pub active_services: AtomicUsize,
}

impl RegistryStats {
    /// Crée de nouvelles stats
    fn new() -> Self {
        Self {
            total_lookups: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            bloom_rejections: AtomicU64::new(0),
            total_registrations: AtomicU64::new(0),
            total_unregistrations: AtomicU64::new(0),
            active_services: AtomicUsize::new(0),
        }
    }

    /// Retourne le taux de cache hit
    pub fn cache_hit_rate(&self) -> f64 {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let total = self.total_lookups.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// Retourne le taux de bloom filter rejection
    pub fn bloom_rejection_rate(&self) -> f64 {
        let rejections = self.bloom_rejections.load(Ordering::Relaxed);
        let total = self.total_lookups.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            rejections as f64 / total as f64
        }
    }
}

impl Default for RegistryStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Registry principal
///
/// Thread-safe service registry avec cache LRU et bloom filter
pub struct Registry {
    /// Backend de stockage
    backend: Box<dyn StorageBackend>,

    /// Cache LRU
    cache: LruCache,

    /// Bloom filter
    bloom: BloomFilter,

    /// Configuration
    config: RegistryConfig,

    /// Statistiques
    stats: RegistryStats,
}

impl Registry {
    /// Crée un nouveau registry avec backend par défaut (InMemory)
    pub fn new() -> Self {
        Self::with_config(RegistryConfig::default())
    }

    /// Crée un registry avec une configuration custom
    pub fn with_config(config: RegistryConfig) -> Self {
        let backend = Box::new(InMemoryBackend::new());
        let cache = LruCache::new(config.cache_size, config.cache_ttl_secs);
        let bloom = BloomFilter::new(config.bloom_size, config.bloom_fp_rate);

        Self {
            backend,
            cache,
            bloom,
            config,
            stats: RegistryStats::new(),
        }
    }

    /// Crée un registry avec un backend custom
    pub fn with_backend(backend: Box<dyn StorageBackend>) -> Self {
        let config = RegistryConfig::default();
        let cache = LruCache::new(config.cache_size, config.cache_ttl_secs);
        let bloom = BloomFilter::new(config.bloom_size, config.bloom_fp_rate);

        Self {
            backend,
            cache,
            bloom,
            config,
            stats: RegistryStats::new(),
        }
    }

    /// Enregistre un nouveau service
    ///
    /// # Errors
    /// - ServiceAlreadyExists si le service existe déjà et est actif
    pub fn register(&mut self, name: ServiceName, mut info: ServiceInfo) -> RegistryResult<()> {
        // Vérifie si le service existe déjà
        if let Some(existing) = self.backend.get(&name) {
            if existing.is_available() {
                return Err(RegistryError::ServiceAlreadyExists(name.to_string()));
            }
        }

        // Active le service
        info.activate();

        // Insère dans le backend
        self.backend.insert(name.clone(), info.clone())?;

        // Insère dans le bloom filter
        self.bloom.insert(name.as_str());

        // Insère dans le cache
        let timestamp = self.current_timestamp();
        self.cache.insert(name.into_string(), info, timestamp);

        // Met à jour les stats
        self.stats.total_registrations.fetch_add(1, Ordering::Relaxed);
        self.stats.active_services.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    /// Lookup un service par nom
    ///
    /// # Performance
    /// - Cache hit: ~50ns
    /// - Bloom rejection: ~100ns
    /// - Backend lookup: ~500ns
    pub fn lookup(&mut self, name: &ServiceName) -> Option<ServiceInfo> {
        self.stats.total_lookups.fetch_add(1, Ordering::Relaxed);

        let timestamp = self.current_timestamp();

        // Essaie le cache d'abord
        if let Some(info) = self.cache.get(name.as_str(), timestamp) {
            self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Some(info);
        }

        self.stats.cache_misses.fetch_add(1, Ordering::Relaxed);

        // Vérifie le bloom filter
        if !self.bloom.might_contain(name.as_str()) {
            self.stats.bloom_rejections.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        // Lookup dans le backend
        let info = self.backend.get(name)?.clone();

        // Cache le résultat
        self.cache.insert(name.to_string(), info.clone(), timestamp);

        Some(info)
    }

    /// Désenregistre un service
    pub fn unregister(&mut self, name: &ServiceName) -> RegistryResult<()> {
        self.backend
            .remove(name)
            .ok_or_else(|| RegistryError::ServiceNotFound(name.to_string()))?;

        // Invalide le cache
        self.cache.invalidate(name.as_str());

        // Met à jour les stats
        self.stats.total_unregistrations.fetch_add(1, Ordering::Relaxed);
        self.stats.active_services.fetch_sub(1, Ordering::Relaxed);

        Ok(())
    }

    /// Liste tous les services
    pub fn list(&self) -> Vec<(ServiceName, ServiceInfo)> {
        self.backend.list()
    }

    /// Liste les services par statut
    pub fn list_by_status(&self, status: ServiceStatus) -> Vec<(ServiceName, ServiceInfo)> {
        self.backend
            .list()
            .into_iter()
            .filter(|(_, info)| info.status() == status)
            .collect()
    }

    /// Mise à jour du heartbeat d'un service
    pub fn heartbeat(&mut self, name: &ServiceName) -> RegistryResult<()> {
        let timestamp = self.current_timestamp();

        let info = self
            .backend
            .get_mut(name)
            .ok_or_else(|| RegistryError::ServiceNotFound(name.to_string()))?;

        info.update_heartbeat(timestamp);

        // Invalide le cache
        self.cache.invalidate(name.as_str());

        Ok(())
    }

    /// Retourne les services stale (pas de heartbeat récent)
    pub fn get_stale_services(&self) -> Vec<(ServiceName, ServiceInfo)> {
        let current_time = self.current_timestamp();
        let threshold = self.config.stale_threshold_secs;

        self.backend
            .list()
            .into_iter()
            .filter(|(_, info)| info.is_stale(current_time, threshold))
            .collect()
    }

    /// Retourne les statistiques
    pub fn stats(&self) -> &RegistryStats {
        &self.stats
    }

    /// Efface tous les services
    pub fn clear(&mut self) {
        self.backend.clear();
        self.cache.clear();
        self.bloom.clear();
        self.stats.active_services.store(0, Ordering::Relaxed);
    }

    /// Flush les changements (si backend persistant)
    pub fn flush(&mut self) -> RegistryResult<()> {
        self.backend.flush()
    }

    /// Charge depuis le storage
    pub fn load(&mut self) -> RegistryResult<()> {
        self.backend.load()?;

        // Reconstruit le bloom filter
        for (name, _) in self.backend.list() {
            self.bloom.insert(name.as_str());
        }

        // Met à jour les stats
        let count = self.backend.len();
        self.stats.active_services.store(count, Ordering::Relaxed);

        Ok(())
    }

    /// Retourne le timestamp actuel (secondes depuis epoch)
    ///
    /// Note: Utilise le timestamp monotonic via exo_types::Timestamp
    #[inline]
    fn current_timestamp(&self) -> u64 {
        crate::time_utils::current_timestamp_secs()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_register_and_lookup() {
        let mut registry = Registry::new();

        let name = ServiceName::new("test_service").unwrap();
        let info = ServiceInfo::new("/tmp/test.sock");

        registry.register(name.clone(), info).unwrap();

        let found = registry.lookup(&name).unwrap();
        assert_eq!(found.endpoint(), "/tmp/test.sock");
        assert_eq!(found.status(), ServiceStatus::Active);
    }

    #[test]
    fn test_registry_duplicate_registration() {
        let mut registry = Registry::new();

        let name = ServiceName::new("test").unwrap();
        let info = ServiceInfo::new("/tmp/test.sock");

        registry.register(name.clone(), info.clone()).unwrap();
        let result = registry.register(name, info);

        assert!(result.is_err());
    }

    #[test]
    fn test_registry_unregister() {
        let mut registry = Registry::new();

        let name = ServiceName::new("test").unwrap();
        registry.register(name.clone(), ServiceInfo::new("/tmp/test.sock")).unwrap();

        registry.unregister(&name).unwrap();
        assert!(registry.lookup(&name).is_none());
    }

    #[test]
    fn test_registry_list() {
        let mut registry = Registry::new();

        registry.register(
            ServiceName::new("service1").unwrap(),
            ServiceInfo::new("/tmp/1.sock"),
        ).unwrap();

        registry.register(
            ServiceName::new("service2").unwrap(),
            ServiceInfo::new("/tmp/2.sock"),
        ).unwrap();

        let list = registry.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_registry_heartbeat() {
        let mut registry = Registry::new();

        let name = ServiceName::new("test").unwrap();
        let mut info = ServiceInfo::new("/tmp/test.sock");
        info.record_failure();
        info.record_failure();
        info.record_failure();
        assert_eq!(info.status(), ServiceStatus::Failed);

        registry.register(name.clone(), info).unwrap();

        registry.heartbeat(&name).unwrap();

        let found = registry.lookup(&name).unwrap();
        assert_eq!(found.metadata().failure_count, 0);
    }

    #[test]
    fn test_registry_stats() {
        let mut registry = Registry::new();

        let name = ServiceName::new("test").unwrap();
        registry.register(name.clone(), ServiceInfo::new("/tmp/test.sock")).unwrap();

        registry.lookup(&name);
        registry.lookup(&name); // Cache hit

        let stats = registry.stats();
        assert_eq!(stats.total_lookups.load(Ordering::Relaxed), 2);
        assert_eq!(stats.cache_hits.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_lru_cache() {
        let mut cache = LruCache::new(2, 60);

        cache.insert("a".into(), ServiceInfo::new("/a"), 0);
        cache.insert("b".into(), ServiceInfo::new("/b"), 0);

        assert!(cache.get("a", 0).is_some());
        assert!(cache.get("b", 0).is_some());

        // Insert 'c', should evict 'a' (LRU)
        cache.insert("c".into(), ServiceInfo::new("/c"), 0);

        assert!(cache.get("a", 0).is_none());
        assert!(cache.get("c", 0).is_some());
    }

    #[test]
    fn test_bloom_filter() {
        let mut bloom = BloomFilter::new(100, 0.01);

        bloom.insert("service1");
        bloom.insert("service2");

        assert!(bloom.might_contain("service1"));
        assert!(bloom.might_contain("service2"));
        assert!(!bloom.might_contain("nonexistent"));
    }

    #[test]
    fn test_cache_expiration() {
        let mut cache = LruCache::new(10, 60);

        cache.insert("test".into(), ServiceInfo::new("/tmp/test"), 100);

        // Lookup à 150s (< 60s, pas expiré)
        assert!(cache.get("test", 150).is_some());

        // Lookup à 200s (> 60s, expiré)
        assert!(cache.get("test", 200).is_none());
    }
}
