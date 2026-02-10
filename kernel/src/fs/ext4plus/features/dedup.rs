//! Block-Level Deduplication
//!
//! Transparent block-level deduplication to save disk space.
//! Uses content-based hashing to identify duplicate blocks.

use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Block hash (Blake3, 32 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BlockHash([u8; 32]);

impl BlockHash {
    /// Compute hash of block data
    fn compute(data: &[u8]) -> Self {
        // In production, would use Blake3
        // For now, use simple hash
        let mut hash = [0u8; 32];
        for (i, &byte) in data.iter().enumerate().take(32) {
            hash[i % 32] ^= byte;
        }
        BlockHash(hash)
    }
}

/// Deduplicated block reference
#[derive(Debug, Clone)]
struct DedupReference {
    /// Physical block number
    physical_block: u64,
    /// Reference count
    refcount: u32,
    /// Size
    size: u32,
}

/// Deduplication Manager
pub struct DeduplicationManager {
    /// Block allocator
    allocator: Arc<super::super::allocation::BlockAllocator>,
    /// Hash -> block reference mapping
    hash_table: Mutex<BTreeMap<BlockHash, DedupReference>>,
    /// Block -> hash mapping (for deletion)
    block_table: Mutex<BTreeMap<u64, BlockHash>>,
    /// Statistics
    stats: DedupStats,
    /// Enable dedup
    enabled: Mutex<bool>,
}

impl DeduplicationManager {
    /// Create new deduplication manager
    pub fn new(allocator: Arc<super::super::allocation::BlockAllocator>) -> Self {
        Self {
            allocator,
            hash_table: Mutex::new(BTreeMap::new()),
            block_table: Mutex::new(BTreeMap::new()),
            stats: DedupStats::new(),
            enabled: Mutex::new(true),
        }
    }

    /// Enable deduplication
    pub fn enable(&self) {
        let mut enabled = self.enabled.lock();
        *enabled = true;
        log::info!("ext4plus: Deduplication enabled");
    }

    /// Disable deduplication
    pub fn disable(&self) {
        let mut enabled = self.enabled.lock();
        *enabled = false;
        log::info!("ext4plus: Deduplication disabled");
    }

    /// Check if deduplication is enabled
    pub fn is_enabled(&self) -> bool {
        *self.enabled.lock()
    }

    /// Write block with deduplication
    pub fn write_block(&self, data: &[u8]) -> FsResult<u64> {
        if !self.is_enabled() {
            // Dedup disabled - allocate new block
            return self.allocator.allocate_block();
        }

        self.stats.writes.fetch_add(1, Ordering::Relaxed);

        // Compute hash
        let hash = BlockHash::compute(data);

        // Check if block already exists
        {
            let mut hash_table = self.hash_table.lock();

            if let Some(dedup_ref) = hash_table.get_mut(&hash) {
                // Block exists - increment refcount
                dedup_ref.refcount += 1;

                self.stats.dedup_hits.fetch_add(1, Ordering::Relaxed);
                self.stats.bytes_saved.fetch_add(data.len() as u64, Ordering::Relaxed);

                log::trace!("ext4plus: Dedup hit for block {} (refcount: {})",
                    dedup_ref.physical_block, dedup_ref.refcount);

                return Ok(dedup_ref.physical_block);
            }
        }

        // Block doesn't exist - allocate new
        let physical_block = self.allocator.allocate_block()?;

        // In production, would write data to block

        // Record in hash table
        {
            let mut hash_table = self.hash_table.lock();
            hash_table.insert(hash, DedupReference {
                physical_block,
                refcount: 1,
                size: data.len() as u32,
            });
        }

        // Record in block table
        {
            let mut block_table = self.block_table.lock();
            block_table.insert(physical_block, hash);
        }

        self.stats.unique_blocks.fetch_add(1, Ordering::Relaxed);

        log::trace!("ext4plus: Allocated new deduplicated block {}", physical_block);

        Ok(physical_block)
    }

    /// Free block (decrements refcount)
    pub fn free_block(&self, logical_block: u64) -> FsResult<()> {
        if !self.is_enabled() {
            // Dedup disabled - free directly
            return self.allocator.free_block(logical_block);
        }

        // Get hash for block
        let hash = {
            let block_table = self.block_table.lock();
            block_table.get(&logical_block).cloned()
        };

        if let Some(hash) = hash {
            let should_free = {
                let mut hash_table = self.hash_table.lock();

                if let Some(dedup_ref) = hash_table.get_mut(&hash) {
                    dedup_ref.refcount -= 1;

                    if dedup_ref.refcount == 0 {
                        // Last reference - remove from hash table
                        hash_table.remove(&hash);
                        log::trace!("ext4plus: Last reference to block {}, freeing", logical_block);
                        true
                    } else {
                        log::trace!("ext4plus: Decremented refcount for block {} (now: {})",
                            logical_block, dedup_ref.refcount);
                        false
                    }
                } else {
                    false
                }
            };

            if should_free {
                // Remove from block table
                {
                    let mut block_table = self.block_table.lock();
                    block_table.remove(&logical_block);
                }

                // Free the physical block
                self.allocator.free_block(logical_block)?;
                self.stats.unique_blocks.fetch_sub(1, Ordering::Relaxed);
            }
        } else {
            // Block not in dedup table - free directly
            self.allocator.free_block(logical_block)?;
        }

        Ok(())
    }

    /// Run deduplication scan
    pub fn scan(&self) -> FsResult<DedupScanResult> {
        log::info!("ext4plus: Starting deduplication scan");

        // In production, would:
        // 1. Scan all inodes
        // 2. Read all blocks
        // 3. Compute hashes
        // 4. Find duplicates
        // 5. Merge duplicate blocks
        // 6. Update extent trees

        let result = DedupScanResult {
            blocks_scanned: 0,
            duplicates_found: 0,
            bytes_saved: 0,
        };

        log::info!("ext4plus: Deduplication scan complete");

        Ok(result)
    }

    /// Get statistics
    pub fn stats(&self) -> DedupStatsSnapshot {
        let hash_table = self.hash_table.lock();
        let total_refs: u32 = hash_table.values().map(|r| r.refcount).sum();

        DedupStatsSnapshot {
            writes: self.stats.writes.load(Ordering::Relaxed),
            dedup_hits: self.stats.dedup_hits.load(Ordering::Relaxed),
            bytes_saved: self.stats.bytes_saved.load(Ordering::Relaxed),
            unique_blocks: self.stats.unique_blocks.load(Ordering::Relaxed),
            total_references: total_refs as u64,
        }
    }

    /// Get deduplication ratio
    pub fn dedup_ratio(&self) -> f64 {
        let stats = self.stats();
        if stats.unique_blocks == 0 {
            1.0
        } else {
            stats.total_references as f64 / stats.unique_blocks as f64
        }
    }

    /// Get space savings
    pub fn space_savings(&self) -> u64 {
        self.stats.bytes_saved.load(Ordering::Relaxed)
    }
}

/// Deduplication statistics snapshot
#[derive(Debug, Clone, Copy)]
pub struct DedupStatsSnapshot {
    pub writes: u64,
    pub dedup_hits: u64,
    pub bytes_saved: u64,
    pub unique_blocks: u64,
    pub total_references: u64,
}

/// Deduplication statistics
#[derive(Debug)]
pub struct DedupStats {
    pub writes: AtomicU64,
    pub dedup_hits: AtomicU64,
    pub bytes_saved: AtomicU64,
    pub unique_blocks: AtomicU64,
    pub total_references: u64,
}

impl Clone for DedupStats {
    fn clone(&self) -> Self {
        Self {
            writes: AtomicU64::new(self.writes.load(Ordering::Relaxed)),
            dedup_hits: AtomicU64::new(self.dedup_hits.load(Ordering::Relaxed)),
            bytes_saved: AtomicU64::new(self.bytes_saved.load(Ordering::Relaxed)),
            unique_blocks: AtomicU64::new(self.unique_blocks.load(Ordering::Relaxed)),
            total_references: self.total_references,
        }
    }
}

impl DedupStats {
    fn new() -> Self {
        Self {
            writes: AtomicU64::new(0),
            dedup_hits: AtomicU64::new(0),
            bytes_saved: AtomicU64::new(0),
            unique_blocks: AtomicU64::new(0),
            total_references: 0,
        }
    }
}

/// Deduplication scan result
#[derive(Debug, Clone, Copy)]
pub struct DedupScanResult {
    pub blocks_scanned: u64,
    pub duplicates_found: u64,
    pub bytes_saved: u64,
}
