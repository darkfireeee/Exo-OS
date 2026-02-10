//! Preallocation Manager
//!
//! Manages block preallocation for files to reduce fragmentation.
//! Preallocates blocks when file grows, reducing allocation overhead.

use super::MultiBlockAllocator;
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;

/// Preallocated extent
#[derive(Debug, Clone)]
struct PreallocExtent {
    /// Starting block
    start_block: u64,
    /// Number of blocks
    count: u32,
    /// Used blocks
    used: u32,
}

impl PreallocExtent {
    fn new(start_block: u64, count: u32) -> Self {
        Self {
            start_block,
            count,
            used: 0,
        }
    }

    fn has_free(&self) -> bool {
        self.used < self.count
    }

    fn get_next(&mut self) -> Option<u64> {
        if self.has_free() {
            let block = self.start_block + self.used as u64;
            self.used += 1;
            Some(block)
        } else {
            None
        }
    }

    fn remaining(&self) -> u32 {
        self.count - self.used
    }
}

/// Preallocation Manager
pub struct PreallocManager {
    /// Preallocated extents per inode
    extents: Mutex<BTreeMap<u64, Vec<PreallocExtent>>>,
    /// Default preallocation size
    default_prealloc_size: u32,
}

impl PreallocManager {
    /// Create new preallocation manager
    pub fn new() -> Self {
        Self {
            extents: Mutex::new(BTreeMap::new()),
            default_prealloc_size: 8, // Preallocate 8 blocks by default
        }
    }

    /// Preallocate blocks for inode
    pub fn preallocate(
        &self,
        inode: u64,
        count: u32,
        allocator: &Arc<MultiBlockAllocator>,
    ) -> FsResult<Vec<u64>> {
        let actual_count = core::cmp::max(count, self.default_prealloc_size);

        // Allocate blocks
        let start_block = allocator.allocate(actual_count)?;

        // Create extent
        let extent = PreallocExtent::new(start_block, actual_count);

        // Calculate blocks
        let mut blocks = Vec::new();
        for i in 0..count {
            blocks.push(start_block + i as u64);
        }

        // Store extent if we allocated more than requested
        if actual_count > count {
            let mut remaining_extent = PreallocExtent::new(start_block, actual_count);
            remaining_extent.used = count;

            let mut extents = self.extents.lock();
            extents.entry(inode)
                .or_insert_with(Vec::new)
                .push(remaining_extent);

            log::debug!("ext4plus: Preallocated {} blocks for inode {} (used {}, remaining {})",
                actual_count, inode, count, actual_count - count);
        } else {
            log::debug!("ext4plus: Preallocated {} blocks for inode {}", count, inode);
        }

        Ok(blocks)
    }

    /// Get next preallocated block for inode
    pub fn get_block(&self, inode: u64) -> Option<u64> {
        let mut extents = self.extents.lock();

        if let Some(inode_extents) = extents.get_mut(&inode) {
            // Try to get block from first extent with free blocks
            for extent in inode_extents.iter_mut() {
                if let Some(block) = extent.get_next() {
                    log::trace!("ext4plus: Using preallocated block {} for inode {}", block, inode);
                    return Some(block);
                }
            }

            // Remove exhausted extents
            inode_extents.retain(|e| e.has_free());

            // Remove entry if no extents left
            if inode_extents.is_empty() {
                extents.remove(&inode);
            }
        }

        None
    }

    /// Free all preallocated blocks for inode
    pub fn free_preallocated(&self, inode: u64) -> Vec<u64> {
        let mut extents = self.extents.lock();

        if let Some(inode_extents) = extents.remove(&inode) {
            let mut blocks = Vec::new();

            for extent in inode_extents {
                // Only free unused blocks
                for i in extent.used..extent.count {
                    blocks.push(extent.start_block + i as u64);
                }
            }

            if !blocks.is_empty() {
                log::debug!("ext4plus: Freed {} preallocated blocks for inode {}",
                    blocks.len(), inode);
            }

            blocks
        } else {
            Vec::new()
        }
    }

    /// Get preallocation statistics for inode
    pub fn get_stats(&self, inode: u64) -> PreallocStats {
        let extents = self.extents.lock();

        if let Some(inode_extents) = extents.get(&inode) {
            let total_preallocated: u32 = inode_extents.iter()
                .map(|e| e.count)
                .sum();

            let total_used: u32 = inode_extents.iter()
                .map(|e| e.used)
                .sum();

            PreallocStats {
                extents: inode_extents.len(),
                total_preallocated,
                total_used,
                total_remaining: total_preallocated - total_used,
            }
        } else {
            PreallocStats {
                extents: 0,
                total_preallocated: 0,
                total_used: 0,
                total_remaining: 0,
            }
        }
    }

    /// Set default preallocation size
    pub fn set_default_size(&mut self, size: u32) {
        self.default_prealloc_size = size;
        log::debug!("ext4plus: Set default preallocation size to {} blocks", size);
    }

    /// Get total preallocated blocks across all inodes
    pub fn total_preallocated(&self) -> u64 {
        let extents = self.extents.lock();
        extents.values()
            .flat_map(|v| v.iter())
            .map(|e| (e.count - e.used) as u64)
            .sum()
    }
}

/// Preallocation statistics
#[derive(Debug, Clone, Copy)]
pub struct PreallocStats {
    pub extents: usize,
    pub total_preallocated: u32,
    pub total_used: u32,
    pub total_remaining: u32,
}
