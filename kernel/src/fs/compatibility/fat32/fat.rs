//! FAT Table Management
//!
//! File Allocation Table operations for cluster chain management.
//! Provides efficient FAT reading, writing, and cluster allocation.

use alloc::vec::Vec;
use alloc::sync::Arc;

use crate::fs::{FsError, FsResult};

use super::Fat32Fs;

/// FAT entry values
pub const FAT_FREE: u32 = 0x00000000;
pub const FAT_RESERVED_MIN: u32 = 0x0FFFFFF0;
pub const FAT_RESERVED_MAX: u32 = 0x0FFFFFF6;
pub const FAT_BAD_CLUSTER: u32 = 0x0FFFFFF7;
pub const FAT_EOC_MIN: u32 = 0x0FFFFFF8;
pub const FAT_EOC_MAX: u32 = 0x0FFFFFFF;

impl Fat32Fs {
    /// Check if FAT entry indicates end of chain
    pub fn is_eoc(entry: u32) -> bool {
        entry >= FAT_EOC_MIN
    }

    /// Check if FAT entry indicates free cluster
    pub fn is_free(entry: u32) -> bool {
        entry == FAT_FREE
    }

    /// Check if FAT entry indicates bad cluster
    pub fn is_bad(entry: u32) -> bool {
        entry == FAT_BAD_CLUSTER
    }

    /// Get cluster chain starting from given cluster
    pub fn get_cluster_chain(&self, start: u32) -> FsResult<Vec<u32>> {
        let mut chain = Vec::new();
        let mut cluster = start;
        let max_clusters = self.boot_sector.total_clusters();

        // Prevent infinite loops
        let mut count = 0;
        const MAX_CHAIN_LENGTH: usize = 1_000_000;

        while cluster >= 2 && cluster < max_clusters {
            if count >= MAX_CHAIN_LENGTH {
                log::error!("FAT32: Cluster chain too long, possible corruption");
                return Err(FsError::InvalidData);
            }

            chain.push(cluster);
            count += 1;

            let entry = self.read_fat_entry(cluster)?;

            if Self::is_eoc(entry) {
                break;
            }

            if Self::is_bad(entry) {
                log::error!("FAT32: Bad cluster in chain: {}", cluster);
                return Err(FsError::InvalidData);
            }

            if Self::is_free(entry) {
                log::error!("FAT32: Free cluster in chain: {}", cluster);
                return Err(FsError::InvalidData);
            }

            cluster = entry;
        }

        Ok(chain)
    }

    /// Allocate cluster chain of given length
    pub fn allocate_cluster_chain(&self, length: usize) -> FsResult<Vec<u32>> {
        if length == 0 {
            return Ok(Vec::new());
        }

        let mut chain = Vec::with_capacity(length);

        // Allocate first cluster
        let first = self.allocate_cluster()?;
        chain.push(first);

        // Allocate remaining clusters and link them
        for _ in 1..length {
            let new_cluster = self.allocate_cluster()?;
            let prev_cluster = *chain.last().unwrap();

            // Link previous cluster to new cluster
            self.write_fat_entry(prev_cluster, new_cluster)?;

            chain.push(new_cluster);
        }

        // Mark last cluster as end of chain
        let last = *chain.last().unwrap();
        self.write_fat_entry(last, FAT_EOC_MAX)?;

        Ok(chain)
    }

    /// Extend cluster chain by given number of clusters
    pub fn extend_cluster_chain(&self, start: u32, additional: usize) -> FsResult<Vec<u32>> {
        if additional == 0 {
            return Ok(Vec::new());
        }

        // Find end of existing chain
        let mut cluster = start;
        let mut prev = start;

        loop {
            prev = cluster;
            let entry = self.read_fat_entry(cluster)?;

            if Self::is_eoc(entry) {
                break;
            }

            cluster = entry;
        }

        // Allocate new clusters
        let mut new_clusters = Vec::with_capacity(additional);

        for i in 0..additional {
            let new_cluster = self.allocate_cluster()?;

            // Link previous to new
            if i == 0 {
                self.write_fat_entry(prev, new_cluster)?;
            } else {
                let prev_new = new_clusters[i - 1];
                self.write_fat_entry(prev_new, new_cluster)?;
            }

            new_clusters.push(new_cluster);
        }

        // Mark last as EOC
        let last = *new_clusters.last().unwrap();
        self.write_fat_entry(last, FAT_EOC_MAX)?;

        Ok(new_clusters)
    }

    /// Truncate cluster chain to given length
    pub fn truncate_cluster_chain(&self, start: u32, new_length: usize) -> FsResult<()> {
        let chain = self.get_cluster_chain(start)?;

        if new_length >= chain.len() {
            return Ok(()); // No truncation needed
        }

        if new_length == 0 {
            // Free entire chain
            return self.free_cluster_chain(start);
        }

        // Mark new end
        let new_end = chain[new_length - 1];
        self.write_fat_entry(new_end, FAT_EOC_MAX)?;

        // Free remaining clusters
        for &cluster in &chain[new_length..] {
            self.write_fat_entry(cluster, FAT_FREE)?;
        }

        Ok(())
    }

    /// Count free clusters
    pub fn count_free_clusters(&self) -> FsResult<u32> {
        let total_clusters = self.boot_sector.total_clusters();
        let mut free_count = 0;

        for cluster in 2..total_clusters {
            let entry = self.read_fat_entry(cluster)?;
            if Self::is_free(entry) {
                free_count += 1;
            }
        }

        Ok(free_count)
    }

    /// Get filesystem statistics
    pub fn statfs(&self) -> FsResult<Fat32Stats> {
        let total_clusters = self.boot_sector.total_clusters();
        let free_clusters = self.count_free_clusters()?;
        let cluster_size = self.cluster_size() as u64;

        Ok(Fat32Stats {
            cluster_size,
            total_clusters: total_clusters as u64,
            free_clusters: free_clusters as u64,
            total_bytes: total_clusters as u64 * cluster_size,
            free_bytes: free_clusters as u64 * cluster_size,
        })
    }
}

/// FAT32 filesystem statistics
#[derive(Debug, Clone, Copy)]
pub struct Fat32Stats {
    pub cluster_size: u64,
    pub total_clusters: u64,
    pub free_clusters: u64,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fat_entry_checks() {
        assert!(Fat32Fs::is_free(FAT_FREE));
        assert!(Fat32Fs::is_eoc(FAT_EOC_MIN));
        assert!(Fat32Fs::is_eoc(FAT_EOC_MAX));
        assert!(Fat32Fs::is_bad(FAT_BAD_CLUSTER));
        assert!(!Fat32Fs::is_free(100));
        assert!(!Fat32Fs::is_eoc(100));
    }
}
