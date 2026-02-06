//! Load balancer pour instances multiples
//!
//! Distribue les requêtes entre plusieurs instances de registry
//! avec différentes stratégies de load balancing.

use alloc::vec::Vec;
use alloc::string::String;
use core::sync::atomic::{AtomicUsize, AtomicU64, Ordering};

use crate::{ServiceName, ServiceInfo, RegistryResult};
use crate::threading::ThreadSafeRegistry;

/// Stratég de load balancing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadBalancingStrategy {
    /// Round-robin simple
    RoundRobin,

    /// Consistent hashing par service name
    ConsistentHash,

    /// Least connections
    LeastConnections,

    /// Weighted round-robin
    WeightedRoundRobin,
}

/// Instance de registry avec métriques
pub struct RegistryInstance {
    /// Registry thread-safe
    registry: ThreadSafeRegistry,

    /// Nom de l'instance
    name: String,

    /// Poids pour weighted load balancing (1-100)
    weight: u32,

    /// Nombre de connexions actives
    active_connections: AtomicUsize,

    /// Total de requêtes traitées
    total_requests: AtomicU64,

    /// Marque comme healthy/unhealthy
    healthy: core::sync::atomic::AtomicBool,
}

impl RegistryInstance {
    /// Crée une nouvelle instance
    pub fn new(name: String, weight: u32) -> Self {
        Self {
            registry: ThreadSafeRegistry::new(),
            name,
            weight,
            active_connections: AtomicUsize::new(0),
            total_requests: AtomicU64::new(0),
            healthy: core::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Incrémente les connexions actives
    pub fn increment_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente les connexions actives
    pub fn decrement_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    /// Retourne le nombre de connexions actives
    pub fn active_connections(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Retourne le total de requêtes
    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    /// Marque comme healthy ou unhealthy
    pub fn set_healthy(&self, healthy: bool) {
        self.healthy.store(healthy, Ordering::Release);
    }

    /// Retourne si l'instance est healthy
    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }
}

/// Load balancer pour registries
pub struct LoadBalancer {
    /// Instances de registry
    instances: Vec<RegistryInstance>,

    /// Stratégie de load balancing
    strategy: LoadBalancingStrategy,

    /// Index du prochain (pour round-robin)
    next_index: AtomicUsize,

    /// Compteur pour weighted round-robin
    weighted_counter: AtomicUsize,
}

impl LoadBalancer {
    /// Crée un nouveau load balancer
    pub fn new(strategy: LoadBalancingStrategy) -> Self {
        Self {
            instances: Vec::new(),
            strategy,
            next_index: AtomicUsize::new(0),
            weighted_counter: AtomicUsize::new(0),
        }
    }

    /// Ajoute une instance
    pub fn add_instance(&mut self, instance: RegistryInstance) {
        self.instances.push(instance);
    }

    /// Sélectionne une instance selon la stratégie
    fn select_instance(&self, name: Option<&ServiceName>) -> Option<&RegistryInstance> {
        // Filtre seulement les instances healthy
        let healthy_instances: Vec<_> = self.instances
            .iter()
            .filter(|i| i.is_healthy())
            .collect();

        if healthy_instances.is_empty() {
            return None;
        }

        match self.strategy {
            LoadBalancingStrategy::RoundRobin => {
                let idx = self.next_index.fetch_add(1, Ordering::Relaxed);
                Some(healthy_instances[idx % healthy_instances.len()])
            }

            LoadBalancingStrategy::ConsistentHash => {
                if let Some(service_name) = name {
                    let hash = self.hash_name(service_name);
                    Some(healthy_instances[hash % healthy_instances.len()])
                } else {
                    // Fallback to round-robin
                    let idx = self.next_index.fetch_add(1, Ordering::Relaxed);
                    Some(healthy_instances[idx % healthy_instances.len()])
                }
            }

            LoadBalancingStrategy::LeastConnections => {
                healthy_instances
                    .iter()
                    .min_by_key(|i| i.active_connections())
                    .copied()
            }

            LoadBalancingStrategy::WeightedRoundRobin => {
                self.weighted_select(&healthy_instances)
            }
        }
    }

    /// Weighted round-robin selection
    fn weighted_select<'a>(
        &self,
        instances: &[&'a RegistryInstance]
    ) -> Option<&'a RegistryInstance> {
        if instances.is_empty() {
            return None;
        }

        let total_weight: u32 = instances.iter().map(|i| i.weight).sum();
        if total_weight == 0 {
            return Some(instances[0]);
        }

        let counter = self.weighted_counter.fetch_add(1, Ordering::Relaxed);
        let mut cumulative = 0;
        let target = (counter % total_weight as usize) as u32;

        for instance in instances {
            cumulative += instance.weight;
            if cumulative > target {
                return Some(instance);
            }
        }

        Some(instances[instances.len() - 1])
    }

    /// Hash un service name
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
        let instance = self.select_instance(Some(name))?;

        instance.increment_connections();
        let result = instance.registry.lookup(name);
        instance.decrement_connections();

        result
    }

    /// Register avec load balancing
    pub fn register(&self, name: ServiceName, info: ServiceInfo) -> RegistryResult<()> {
        let instance = self.select_instance(Some(&name))
            .ok_or_else(|| crate::RegistryError::StorageError("No healthy instances".into()))?;

        instance.increment_connections();
        let result = instance.registry.register(name, info);
        instance.decrement_connections();

        result
    }

    /// Unregister avec load balancing
    pub fn unregister(&self, name: &ServiceName) -> RegistryResult<()> {
        let instance = self.select_instance(Some(name))
            .ok_or_else(|| crate::RegistryError::StorageError("No healthy instances".into()))?;

        instance.increment_connections();
        let result = instance.registry.unregister(name);
        instance.decrement_connections();

        result
    }

    /// Heartbeat avec load balancing
    pub fn heartbeat(&self, name: &ServiceName) -> RegistryResult<()> {
        let instance = self.select_instance(Some(name))
            .ok_or_else(|| crate::RegistryError::StorageError("No healthy instances".into()))?;

        instance.increment_connections();
        let result = instance.registry.heartbeat(name);
        instance.decrement_connections();

        result
    }

    /// Health check sur toutes les instances
    pub fn health_check(&self) -> Vec<(String, bool, usize, u64)> {
        self.instances
            .iter()
            .map(|i| (
                i.name.clone(),
                i.is_healthy(),
                i.active_connections(),
                i.total_requests(),
            ))
            .collect()
    }

    /// Nombre total d'instances
    pub fn total_instances(&self) -> usize {
        self.instances.len()
    }

    /// Nombre d'instances healthy
    pub fn healthy_instances(&self) -> usize {
        self.instances.iter().filter(|i| i.is_healthy()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_balancer_round_robin() {
        let mut lb = LoadBalancer::new(LoadBalancingStrategy::RoundRobin);

        lb.add_instance(RegistryInstance::new("instance1".into(), 1));
        lb.add_instance(RegistryInstance::new("instance2".into(), 1));
        lb.add_instance(RegistryInstance::new("instance3".into(), 1));

        // Register services
        for i in 0..10 {
            let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
            let info = ServiceInfo::new(&alloc::format!("/tmp/service_{}.sock", i));
            lb.register(name, info).unwrap();
        }

        // Check lookup
        let name = ServiceName::new("service_5").unwrap();
        let found = lb.lookup(&name);
        assert!(found.is_some());
    }

    #[test]
    fn test_load_balancer_least_connections() {
        let mut lb = LoadBalancer::new(LoadBalancingStrategy::LeastConnections);

        lb.add_instance(RegistryInstance::new("instance1".into(), 1));
        lb.add_instance(RegistryInstance::new("instance2".into(), 1));

        assert_eq!(lb.healthy_instances(), 2);
    }

    #[test]
    fn test_load_balancer_weighted() {
        let mut lb = LoadBalancer::new(LoadBalancingStrategy::WeightedRoundRobin);

        lb.add_instance(RegistryInstance::new("heavy".into(), 80));
        lb.add_instance(RegistryInstance::new("light".into(), 20));

        // Register 100 services - ~80 should go to heavy, ~20 to light
        for i in 0..100 {
            let name = ServiceName::new(&alloc::format!("service_{}", i)).unwrap();
            let info = ServiceInfo::new(&alloc::format!("/tmp/service_{}.sock", i));
            lb.register(name, info).unwrap();
        }

        let health = lb.health_check();
        let heavy_requests = health.iter().find(|(n, _, _, _)| n == "heavy").unwrap().3;
        let light_requests = health.iter().find(|(n, _, _, _)| n == "light").unwrap().3;

        // Heavy should have ~80% of requests
        assert!(heavy_requests > light_requests);
    }

    #[test]
    fn test_health_check() {
        let mut lb = LoadBalancer::new(LoadBalancingStrategy::RoundRobin);

        lb.add_instance(RegistryInstance::new("instance1".into(), 1));
        let mut instance2 = RegistryInstance::new("instance2".into(), 1);
        instance2.set_healthy(false);
        lb.add_instance(instance2);

        assert_eq!(lb.total_instances(), 2);
        assert_eq!(lb.healthy_instances(), 1);
    }
}
