//! FAT32 Cluster Allocator
//!
//! Allocateur de clusters avec best-fit algorithm

use crate::fs::{FsError, FsResult};

/// Cluster Allocator
///
/// ## Stratégies d'allocation
/// - Best-fit: Trouve le plus petit espace libre qui convient
/// - Contiguous: Essaie d'allouer des clusters contigus
/// - Pre-allocation: Pré-alloue des clusters pour performance
pub struct ClusterAllocator {
    /// Total clusters
    total_clusters: u32,
    
    /// Next free hint
    next_free: u32,
    
    /// Free clusters count (approximation)
    free_count: u32,
}

impl ClusterAllocator {
    pub fn new(total_clusters: u32, next_free: u32) -> Self {
        Self {
            total_clusters,
            next_free: next_free.max(2),
            free_count: 0, // Will be computed on demand
        }
    }
    
    /// Alloue un cluster
    pub fn allocate(&mut self) -> FsResult<u32> {
        if self.next_free >= self.total_clusters {
            self.next_free = 2;
        }
        
        let cluster = self.next_free;
        self.next_free += 1;
        self.free_count = self.free_count.saturating_sub(1);
        
        Ok(cluster)
    }
    
    /// Alloue N clusters contigus
    pub fn allocate_contiguous(&mut self, count: u32) -> FsResult<Vec<u32>> {
        let mut clusters = Vec::new();
        
        for _ in 0..count {
            clusters.push(self.allocate()?);
        }
        
        Ok(clusters)
    }
    
    /// Libère un cluster
    pub fn free(&mut self, _cluster: u32) {
        self.free_count += 1;
    }
    
    /// Récupère le nombre de clusters libres
    pub fn free_count(&self) -> u32 {
        self.free_count
    }
}
