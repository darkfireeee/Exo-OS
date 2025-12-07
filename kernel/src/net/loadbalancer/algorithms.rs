//! # Load Balancer Algorithms
//! 
//! Load balancing strategies with:
//! - Round Robin
//! - Least Connections
//! - IP Hash (session affinity)
//! - Weighted Round Robin

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::sync::SpinLock;

/// Backend server
#[derive(Debug, Clone)]
pub struct Backend {
    pub id: u32,
    pub address: [u8; 4],
    pub port: u16,
    pub weight: u32,
    pub active_connections: AtomicU64,
    pub total_requests: AtomicU64,
    pub healthy: bool,
}

impl Backend {
    pub fn new(id: u32, address: [u8; 4], port: u16, weight: u32) -> Self {
        Self {
            id,
            address,
            port,
            weight,
            active_connections: AtomicU64::new(0),
            total_requests: AtomicU64::new(0),
            healthy: true,
        }
    }
    
    pub fn connections(&self) -> u64 {
        self.active_connections.load(Ordering::Relaxed)
    }
    
    pub fn increment_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn decrement_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Load balancing algorithm
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LbAlgorithm {
    RoundRobin,
    LeastConnections,
    IpHash,
    WeightedRoundRobin,
}

/// Round Robin load balancer
pub struct RoundRobinLb {
    backends: SpinLock<Vec<Backend>>,
    current: AtomicUsize,
}

impl RoundRobinLb {
    pub fn new() -> Self {
        Self {
            backends: SpinLock::new(Vec::new()),
            current: AtomicUsize::new(0),
        }
    }
    
    pub fn add_backend(&self, backend: Backend) {
        let mut backends = self.backends.lock();
        backends.push(backend);
    }
    
    pub fn select_backend(&self) -> Option<u32> {
        let backends = self.backends.lock();
        if backends.is_empty() {
            return None;
        }
        
        let healthy: Vec<_> = backends.iter()
            .filter(|b| b.healthy)
            .collect();
        
        if healthy.is_empty() {
            return None;
        }
        
        let index = self.current.fetch_add(1, Ordering::Relaxed) % healthy.len();
        Some(healthy[index].id)
    }
}

/// Least Connections load balancer
pub struct LeastConnectionsLb {
    backends: SpinLock<Vec<Backend>>,
}

impl LeastConnectionsLb {
    pub fn new() -> Self {
        Self {
            backends: SpinLock::new(Vec::new()),
        }
    }
    
    pub fn add_backend(&self, backend: Backend) {
        let mut backends = self.backends.lock();
        backends.push(backend);
    }
    
    pub fn select_backend(&self) -> Option<u32> {
        let backends = self.backends.lock();
        
        backends.iter()
            .filter(|b| b.healthy)
            .min_by_key(|b| b.connections())
            .map(|b| b.id)
    }
}

/// IP Hash load balancer (session affinity)
pub struct IpHashLb {
    backends: SpinLock<Vec<Backend>>,
}

impl IpHashLb {
    pub fn new() -> Self {
        Self {
            backends: SpinLock::new(Vec::new()),
        }
    }
    
    pub fn add_backend(&self, backend: Backend) {
        let mut backends = self.backends.lock();
        backends.push(backend);
    }
    
    pub fn select_backend(&self, client_ip: [u8; 4]) -> Option<u32> {
        let backends = self.backends.lock();
        
        let healthy: Vec<_> = backends.iter()
            .filter(|b| b.healthy)
            .collect();
        
        if healthy.is_empty() {
            return None;
        }
        
        // Hash client IP
        let hash = ip_hash(client_ip);
        let index = (hash as usize) % healthy.len();
        
        Some(healthy[index].id)
    }
}

/// Weighted Round Robin load balancer
pub struct WeightedRoundRobinLb {
    backends: SpinLock<Vec<Backend>>,
    current_backend: AtomicUsize,
    current_weight: AtomicU32,
}

impl WeightedRoundRobinLb {
    pub fn new() -> Self {
        Self {
            backends: SpinLock::new(Vec::new()),
            current_backend: AtomicUsize::new(0),
            current_weight: AtomicU32::new(0),
        }
    }
    
    pub fn add_backend(&self, backend: Backend) {
        let mut backends = self.backends.lock();
        backends.push(backend);
    }
    
    pub fn select_backend(&self) -> Option<u32> {
        let backends = self.backends.lock();
        
        let healthy: Vec<_> = backends.iter()
            .filter(|b| b.healthy)
            .collect();
        
        if healthy.is_empty() {
            return None;
        }
        
        // Find max weight
        let max_weight = healthy.iter().map(|b| b.weight).max().unwrap_or(1);
        
        loop {
            let idx = self.current_backend.load(Ordering::Relaxed);
            let backend = &healthy[idx % healthy.len()];
            
            let current_weight = self.current_weight.load(Ordering::Relaxed);
            
            if current_weight >= backend.weight {
                // Move to next backend
                self.current_backend.store((idx + 1) % healthy.len(), Ordering::Relaxed);
                self.current_weight.store(0, Ordering::Relaxed);
                continue;
            }
            
            // Use this backend
            self.current_weight.fetch_add(1, Ordering::Relaxed);
            return Some(backend.id);
        }
    }
}

/// Generic load balancer
pub struct LoadBalancer {
    algorithm: LbAlgorithm,
    round_robin: Option<RoundRobinLb>,
    least_connections: Option<LeastConnectionsLb>,
    ip_hash: Option<IpHashLb>,
    weighted_rr: Option<WeightedRoundRobinLb>,
}

impl LoadBalancer {
    pub fn new(algorithm: LbAlgorithm) -> Self {
        let mut lb = Self {
            algorithm,
            round_robin: None,
            least_connections: None,
            ip_hash: None,
            weighted_rr: None,
        };
        
        match algorithm {
            LbAlgorithm::RoundRobin => {
                lb.round_robin = Some(RoundRobinLb::new());
            }
            LbAlgorithm::LeastConnections => {
                lb.least_connections = Some(LeastConnectionsLb::new());
            }
            LbAlgorithm::IpHash => {
                lb.ip_hash = Some(IpHashLb::new());
            }
            LbAlgorithm::WeightedRoundRobin => {
                lb.weighted_rr = Some(WeightedRoundRobinLb::new());
            }
        }
        
        lb
    }
    
    pub fn add_backend(&self, backend: Backend) {
        match self.algorithm {
            LbAlgorithm::RoundRobin => {
                if let Some(ref lb) = self.round_robin {
                    lb.add_backend(backend);
                }
            }
            LbAlgorithm::LeastConnections => {
                if let Some(ref lb) = self.least_connections {
                    lb.add_backend(backend);
                }
            }
            LbAlgorithm::IpHash => {
                if let Some(ref lb) = self.ip_hash {
                    lb.add_backend(backend);
                }
            }
            LbAlgorithm::WeightedRoundRobin => {
                if let Some(ref lb) = self.weighted_rr {
                    lb.add_backend(backend);
                }
            }
        }
    }
    
    pub fn select_backend(&self, client_ip: Option<[u8; 4]>) -> Option<u32> {
        match self.algorithm {
            LbAlgorithm::RoundRobin => {
                self.round_robin.as_ref()?.select_backend()
            }
            LbAlgorithm::LeastConnections => {
                self.least_connections.as_ref()?.select_backend()
            }
            LbAlgorithm::IpHash => {
                let ip = client_ip?;
                self.ip_hash.as_ref()?.select_backend(ip)
            }
            LbAlgorithm::WeightedRoundRobin => {
                self.weighted_rr.as_ref()?.select_backend()
            }
        }
    }
}

/// IP hash function
fn ip_hash(ip: [u8; 4]) -> u32 {
    let mut hash: u32 = 2166136261;
    for byte in ip {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_round_robin() {
        let lb = RoundRobinLb::new();
        
        lb.add_backend(Backend::new(1, [192, 168, 1, 10], 8080, 1));
        lb.add_backend(Backend::new(2, [192, 168, 1, 11], 8080, 1));
        lb.add_backend(Backend::new(3, [192, 168, 1, 12], 8080, 1));
        
        // Should cycle through backends
        assert_eq!(lb.select_backend(), Some(1));
        assert_eq!(lb.select_backend(), Some(2));
        assert_eq!(lb.select_backend(), Some(3));
        assert_eq!(lb.select_backend(), Some(1));
    }
    
    #[test]
    fn test_ip_hash_consistency() {
        let lb = IpHashLb::new();
        
        lb.add_backend(Backend::new(1, [192, 168, 1, 10], 8080, 1));
        lb.add_backend(Backend::new(2, [192, 168, 1, 11], 8080, 1));
        
        let client_ip = [10, 0, 0, 1];
        
        // Same client should always get same backend
        let backend1 = lb.select_backend(client_ip);
        let backend2 = lb.select_backend(client_ip);
        assert_eq!(backend1, backend2);
    }
}
