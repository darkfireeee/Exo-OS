//! NUMA (Non-Uniform Memory Access) Awareness
//!
//! Phase 2d: Basic NUMA topology and distance metrics
//!
//! Provides:
//! - NUMA node detection
//! - Inter-node distance metrics
//! - NUMA-aware memory allocation hints

use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::vec::Vec;
use spin::Mutex;

/// Maximum number of NUMA nodes supported
pub const MAX_NUMA_NODES: usize = 8;

/// NUMA distance cost (relative, higher = more expensive)
pub type NumaDistance = u8;

/// Special distance values
pub const NUMA_DISTANCE_LOCAL: NumaDistance = 10;  // Local node
pub const NUMA_DISTANCE_REMOTE: NumaDistance = 20; // Remote node (same socket)
pub const NUMA_DISTANCE_FAR: NumaDistance = 30;    // Far node (different socket)

/// NUMA node information
#[derive(Debug)]
pub struct NumaNode {
    /// Node ID
    pub id: usize,
    
    /// CPUs belonging to this node
    pub cpus: Vec<usize>,
    
    /// Total memory in this node (bytes)
    pub total_memory: u64,
    
    /// Free memory in this node (bytes)
    pub free_memory: AtomicUsize,
    
    /// Memory allocations from this node
    pub allocations: AtomicUsize,
}

impl NumaNode {
    pub fn new(id: usize, cpus: Vec<usize>, total_memory: u64) -> Self {
        Self {
            id,
            cpus,
            total_memory,
            free_memory: AtomicUsize::new(total_memory as usize),
            allocations: AtomicUsize::new(0),
        }
    }
    
    /// Check if CPU belongs to this node
    pub fn contains_cpu(&self, cpu: usize) -> bool {
        self.cpus.contains(&cpu)
    }
    
    /// Allocate memory from this node
    pub fn allocate(&self, size: usize) -> bool {
        let current = self.free_memory.load(Ordering::Acquire);
        if current >= size {
            self.free_memory.fetch_sub(size, Ordering::Release);
            self.allocations.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
    
    /// Free memory back to this node
    pub fn deallocate(&self, size: usize) {
        self.free_memory.fetch_add(size, Ordering::Release);
        self.allocations.fetch_sub(1, Ordering::Relaxed);
    }
    
    /// Get utilization ratio (0.0 = empty, 1.0 = full)
    pub fn utilization(&self) -> f32 {
        let free = self.free_memory.load(Ordering::Relaxed) as f32;
        let total = self.total_memory as f32;
        1.0 - (free / total)
    }
}

/// NUMA topology
pub struct NumaTopology {
    /// NUMA nodes
    nodes: Mutex<Vec<NumaNode>>,
    
    /// Distance matrix [from][to]
    distances: [[NumaDistance; MAX_NUMA_NODES]; MAX_NUMA_NODES],
    
    /// Number of active NUMA nodes
    node_count: AtomicUsize,
}

impl NumaTopology {
    /// Create empty NUMA topology
    pub const fn new() -> Self {
        Self {
            nodes: Mutex::new(Vec::new()),
            distances: [[NUMA_DISTANCE_FAR; MAX_NUMA_NODES]; MAX_NUMA_NODES],
            node_count: AtomicUsize::new(0),
        }
    }
    
    /// Initialize NUMA topology (Phase 2d: Simple uniform model)
    ///
    /// For now, assume uniform system (UMA) with single node
    pub fn init(&self, num_cpus: usize, total_memory: u64) {
        let mut nodes = self.nodes.lock();
        
        // Create single NUMA node with all CPUs
        let cpus: Vec<usize> = (0..num_cpus).collect();
        let node = NumaNode::new(0, cpus, total_memory);
        
        nodes.push(node);
        self.node_count.store(1, Ordering::Release);
        
        crate::logger::info(&alloc::format!(
            "[NUMA] Initialized: 1 node, {} CPUs, {} MB memory",
            num_cpus,
            total_memory / (1024 * 1024)
        ));
    }
    
    /// Get number of NUMA nodes
    pub fn node_count(&self) -> usize {
        self.node_count.load(Ordering::Acquire)
    }
    
    /// Get NUMA node for a CPU
    pub fn node_for_cpu(&self, cpu: usize) -> Option<usize> {
        let nodes = self.nodes.lock();
        for node in nodes.iter() {
            if node.contains_cpu(cpu) {
                return Some(node.id);
            }
        }
        None
    }
    
    /// Get distance between two NUMA nodes
    pub fn distance(&self, from: usize, to: usize) -> NumaDistance {
        if from >= MAX_NUMA_NODES || to >= MAX_NUMA_NODES {
            return NUMA_DISTANCE_FAR;
        }
        
        if from == to {
            NUMA_DISTANCE_LOCAL
        } else {
            self.distances[from][to]
        }
    }
    
    /// Set distance between two NUMA nodes
    pub fn set_distance(&mut self, from: usize, to: usize, distance: NumaDistance) {
        if from < MAX_NUMA_NODES && to < MAX_NUMA_NODES {
            self.distances[from][to] = distance;
            self.distances[to][from] = distance; // Symmetric
        }
    }
    
    /// Find best NUMA node for allocation (least loaded)
    pub fn best_node_for_allocation(&self, size: usize) -> Option<usize> {
        let nodes = self.nodes.lock();
        
        let mut best_node = None;
        let mut best_util = 1.0f32;
        
        for node in nodes.iter() {
            if node.free_memory.load(Ordering::Relaxed) >= size {
                let util = node.utilization();
                if util < best_util {
                    best_util = util;
                    best_node = Some(node.id);
                }
            }
        }
        
        best_node
    }
    
    /// Find best NUMA node for CPU (prefer local node)
    pub fn best_node_for_cpu(&self, cpu: usize, size: usize) -> Option<usize> {
        // First try local node
        if let Some(local_node) = self.node_for_cpu(cpu) {
            let nodes = self.nodes.lock();
            if let Some(node) = nodes.get(local_node) {
                if node.free_memory.load(Ordering::Relaxed) >= size {
                    return Some(local_node);
                }
            }
        }
        
        // Fallback to best node
        self.best_node_for_allocation(size)
    }
    
    /// Allocate from specific NUMA node
    pub fn allocate(&self, node_id: usize, size: usize) -> bool {
        let nodes = self.nodes.lock();
        if let Some(node) = nodes.get(node_id) {
            node.allocate(size)
        } else {
            false
        }
    }
    
    /// Deallocate from specific NUMA node
    pub fn deallocate(&self, node_id: usize, size: usize) {
        let nodes = self.nodes.lock();
        if let Some(node) = nodes.get(node_id) {
            node.deallocate(size);
        }
    }
    
    /// Get NUMA statistics
    pub fn stats(&self) -> NumaStats {
        let nodes = self.nodes.lock();
        let node_count = nodes.len();
        
        let mut total_memory = 0u64;
        let mut free_memory = 0u64;
        let mut total_allocations = 0usize;
        
        for node in nodes.iter() {
            total_memory += node.total_memory;
            free_memory += node.free_memory.load(Ordering::Relaxed) as u64;
            total_allocations += node.allocations.load(Ordering::Relaxed);
        }
        
        NumaStats {
            node_count,
            total_memory,
            free_memory,
            total_allocations,
        }
    }
}

/// NUMA statistics
#[derive(Debug, Clone, Copy)]
pub struct NumaStats {
    pub node_count: usize,
    pub total_memory: u64,
    pub free_memory: u64,
    pub total_allocations: usize,
}

/// Global NUMA topology
pub static NUMA_TOPOLOGY: NumaTopology = NumaTopology::new();

/// Initialize NUMA subsystem
pub fn init(num_cpus: usize, total_memory: u64) {
    NUMA_TOPOLOGY.init(num_cpus, total_memory);
}

/// Get NUMA node for current CPU
pub fn current_node() -> Option<usize> {
    let cpu_id = crate::scheduler::smp_init::current_cpu_id();
    NUMA_TOPOLOGY.node_for_cpu(cpu_id)
}

/// Allocate memory from preferred NUMA node
pub fn numa_alloc(size: usize, preferred_node: Option<usize>) -> Option<usize> {
    if let Some(node) = preferred_node {
        if NUMA_TOPOLOGY.allocate(node, size) {
            return Some(node);
        }
    }
    
    // Fallback to best available node
    if let Some(node) = NUMA_TOPOLOGY.best_node_for_allocation(size) {
        if NUMA_TOPOLOGY.allocate(node, size) {
            return Some(node);
        }
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_numa_node() {
        let node = NumaNode::new(0, vec![0, 1, 2, 3], 1024 * 1024 * 1024);
        
        assert!(node.contains_cpu(0));
        assert!(node.contains_cpu(3));
        assert!(!node.contains_cpu(4));
        
        assert!(node.allocate(1024));
        assert_eq!(node.allocations.load(Ordering::Relaxed), 1);
        
        node.deallocate(1024);
        assert_eq!(node.allocations.load(Ordering::Relaxed), 0);
    }
    
    #[test]
    fn test_numa_topology() {
        let topo = NumaTopology::new();
        topo.init(4, 1024 * 1024 * 1024);
        
        assert_eq!(topo.node_count(), 1);
        assert_eq!(topo.node_for_cpu(0), Some(0));
        assert_eq!(topo.distance(0, 0), NUMA_DISTANCE_LOCAL);
    }
}
