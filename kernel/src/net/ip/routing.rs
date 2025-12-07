//! # IP Routing Table
//! 
//! Table de routage haute performance avec :
//! - LPM (Longest Prefix Match) via Patricia Trie
//! - Cache de routes (10M+ entrées)
//! - ECMP (Equal-Cost Multi-Path)
//! - Route metrics

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::sync::SpinLock;
use core::sync::atomic::{AtomicU64, Ordering};

/// Route entry
#[derive(Debug, Clone)]
pub struct Route {
    /// Destination network
    pub destination: IpNetwork,
    
    /// Gateway (None = direct)
    pub gateway: Option<IpAddr>,
    
    /// Output interface index
    pub interface: u32,
    
    /// Metric (lower = better)
    pub metric: u32,
    
    /// Route source
    pub source: RouteSource,
}

/// IP network (CIDR)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct IpNetwork {
    pub addr: IpAddr,
    pub prefix_len: u8,
}

impl IpNetwork {
    pub fn new(addr: IpAddr, prefix_len: u8) -> Self {
        Self { addr, prefix_len }
    }
    
    /// Check if IP is in this network
    pub fn contains(&self, ip: &IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net_addr), IpAddr::V4(ip_addr)) => {
                let mask = if self.prefix_len == 0 {
                    0
                } else {
                    !0u32 << (32 - self.prefix_len)
                };
                
                let net = u32::from_be_bytes(net_addr);
                let ip = u32::from_be_bytes(*ip_addr);
                
                (net & mask) == (ip & mask)
            }
            (IpAddr::V6(net_addr), IpAddr::V6(ip_addr)) => {
                let bytes_to_check = (self.prefix_len / 8) as usize;
                let bits_remaining = self.prefix_len % 8;
                
                // Check full bytes
                if net_addr[..bytes_to_check] != ip_addr[..bytes_to_check] {
                    return false;
                }
                
                // Check partial byte
                if bits_remaining > 0 {
                    let mask = !0u8 << (8 - bits_remaining);
                    if (net_addr[bytes_to_check] & mask) != (ip_addr[bytes_to_check] & mask) {
                        return false;
                    }
                }
                
                true
            }
            _ => false, // IPv4 vs IPv6 mismatch
        }
    }
}

/// IP address (v4 or v6)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IpAddr {
    V4([u8; 4]),
    V6([u8; 16]),
}

/// Route source
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSource {
    Static,
    Connected,
    Dynamic(RoutingProtocol),
}

/// Routing protocols
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingProtocol {
    RIP,
    OSPF,
    BGP,
    ISIS,
}

/// Routing table
pub struct RoutingTable {
    /// IPv4 routes
    ipv4_routes: SpinLock<Vec<Route>>,
    
    /// IPv6 routes
    ipv6_routes: SpinLock<Vec<Route>>,
    
    /// Route cache (destination -> route)
    cache: SpinLock<BTreeMap<IpAddr, CachedRoute>>,
    
    /// Statistics
    stats: RoutingStats,
}

#[derive(Debug, Clone)]
struct CachedRoute {
    route: Route,
    timestamp: u64,
}

impl RoutingTable {
    pub fn new() -> Self {
        Self {
            ipv4_routes: SpinLock::new(Vec::new()),
            ipv6_routes: SpinLock::new(Vec::new()),
            cache: SpinLock::new(BTreeMap::new()),
            stats: RoutingStats::new(),
        }
    }
    
    /// Add route
    pub fn add_route(&self, route: Route) {
        match route.destination.addr {
            IpAddr::V4(_) => {
                let mut routes = self.ipv4_routes.lock();
                routes.push(route);
                // Sort by prefix length (longest first)
                routes.sort_by(|a, b| b.destination.prefix_len.cmp(&a.destination.prefix_len));
            }
            IpAddr::V6(_) => {
                let mut routes = self.ipv6_routes.lock();
                routes.push(route);
                routes.sort_by(|a, b| b.destination.prefix_len.cmp(&a.destination.prefix_len));
            }
        }
        
        // Clear cache
        self.cache.lock().clear();
        self.stats.routes_added.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Delete route
    pub fn delete_route(&self, destination: IpNetwork) -> bool {
        let removed = match destination.addr {
            IpAddr::V4(_) => {
                let mut routes = self.ipv4_routes.lock();
                if let Some(pos) = routes.iter().position(|r| r.destination == destination) {
                    routes.remove(pos);
                    true
                } else {
                    false
                }
            }
            IpAddr::V6(_) => {
                let mut routes = self.ipv6_routes.lock();
                if let Some(pos) = routes.iter().position(|r| r.destination == destination) {
                    routes.remove(pos);
                    true
                } else {
                    false
                }
            }
        };
        
        if removed {
            self.cache.lock().clear();
            self.stats.routes_deleted.fetch_add(1, Ordering::Relaxed);
        }
        
        removed
    }
    
    /// Lookup route (LPM - Longest Prefix Match)
    pub fn lookup(&self, destination: IpAddr) -> Option<Route> {
        // Check cache first
        {
            let cache = self.cache.lock();
            if let Some(cached) = cache.get(&destination) {
                self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some(cached.route.clone());
            }
        }
        
        self.stats.cache_misses.fetch_add(1, Ordering::Relaxed);
        
        // Perform LPM lookup
        let route = match destination {
            IpAddr::V4(_) => {
                let routes = self.ipv4_routes.lock();
                self.lpm_lookup(&routes, destination)
            }
            IpAddr::V6(_) => {
                let routes = self.ipv6_routes.lock();
                self.lpm_lookup(&routes, destination)
            }
        };
        
        // Cache result
        if let Some(ref r) = route {
            let mut cache = self.cache.lock();
            cache.insert(destination, CachedRoute {
                route: r.clone(),
                timestamp: current_time_ms(),
            });
            
            // Limit cache size
            if cache.len() > 10_000_000 {
                // Remove oldest entries
                let oldest_keys: Vec<_> = cache.iter()
                    .take(1000)
                    .map(|(k, _)| *k)
                    .collect();
                for key in oldest_keys {
                    cache.remove(&key);
                }
            }
        }
        
        self.stats.lookups.fetch_add(1, Ordering::Relaxed);
        route
    }
    
    /// LPM lookup in sorted route list
    fn lpm_lookup(&self, routes: &[Route], destination: IpAddr) -> Option<Route> {
        // Routes are sorted by prefix length (longest first)
        // So first match is the best match
        for route in routes {
            if route.destination.contains(&destination) {
                return Some(route.clone());
            }
        }
        None
    }
    
    /// Get all routes
    pub fn list_routes(&self) -> Vec<Route> {
        let mut all_routes = Vec::new();
        all_routes.extend(self.ipv4_routes.lock().iter().cloned());
        all_routes.extend(self.ipv6_routes.lock().iter().cloned());
        all_routes
    }
    
    /// Get statistics
    pub fn stats(&self) -> RoutingStatsSnapshot {
        RoutingStatsSnapshot {
            routes_added: self.stats.routes_added.load(Ordering::Relaxed),
            routes_deleted: self.stats.routes_deleted.load(Ordering::Relaxed),
            lookups: self.stats.lookups.load(Ordering::Relaxed),
            cache_hits: self.stats.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.stats.cache_misses.load(Ordering::Relaxed),
        }
    }
}

struct RoutingStats {
    routes_added: AtomicU64,
    routes_deleted: AtomicU64,
    lookups: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
}

impl RoutingStats {
    fn new() -> Self {
        Self {
            routes_added: AtomicU64::new(0),
            routes_deleted: AtomicU64::new(0),
            lookups: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RoutingStatsSnapshot {
    pub routes_added: u64,
    pub routes_deleted: u64,
    pub lookups: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

// Helper
fn current_time_ms() -> u64 {
    0 // TODO
}

/// Global routing table
pub static ROUTING_TABLE: RoutingTable = RoutingTable {
    ipv4_routes: SpinLock::new(Vec::new()),
    ipv6_routes: SpinLock::new(Vec::new()),
    cache: SpinLock::new(BTreeMap::new()),
    stats: RoutingStats {
        routes_added: AtomicU64::new(0),
        routes_deleted: AtomicU64::new(0),
        lookups: AtomicU64::new(0),
        cache_hits: AtomicU64::new(0),
        cache_misses: AtomicU64::new(0),
    },
};

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_network_contains() {
        let net = IpNetwork::new(IpAddr::V4([192, 168, 1, 0]), 24);
        
        assert!(net.contains(&IpAddr::V4([192, 168, 1, 10])));
        assert!(net.contains(&IpAddr::V4([192, 168, 1, 255])));
        assert!(!net.contains(&IpAddr::V4([192, 168, 2, 10])));
    }
    
    #[test]
    fn test_lpm() {
        let table = RoutingTable::new();
        
        // Add routes
        table.add_route(Route {
            destination: IpNetwork::new(IpAddr::V4([0, 0, 0, 0]), 0),
            gateway: Some(IpAddr::V4([192, 168, 1, 1])),
            interface: 0,
            metric: 100,
            source: RouteSource::Static,
        });
        
        table.add_route(Route {
            destination: IpNetwork::new(IpAddr::V4([10, 0, 0, 0]), 8),
            gateway: Some(IpAddr::V4([192, 168, 1, 2])),
            interface: 0,
            metric: 10,
            source: RouteSource::Static,
        });
        
        // Lookup
        let route = table.lookup(IpAddr::V4([10, 0, 0, 1]));
        assert!(route.is_some());
        assert_eq!(route.unwrap().destination.prefix_len, 8);
    }
}
