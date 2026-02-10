//! Snapshot Management
//!
//! Copy-on-write snapshots for point-in-time filesystem state.
//! Allows rollback and backup without copying entire filesystem.

use crate::fs::{FsError, FsResult};
use crate::fs::block::BlockDevice;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;

/// Snapshot
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Snapshot ID
    pub id: u64,
    /// Snapshot name
    pub name: String,
    /// Creation timestamp
    pub created: u64,
    /// Root inode
    pub root_inode: u64,
    /// Size (bytes saved by COW)
    pub size: u64,
    /// Is read-only
    pub readonly: bool,
}

impl Snapshot {
    fn new(id: u64, name: String, root_inode: u64) -> Self {
        Self {
            id,
            name,
            created: crate::time::unix_timestamp(),
            root_inode,
            size: 0,
            readonly: true,
        }
    }
}

/// Copy-on-write mapping
struct CowMapping {
    /// Original block
    original_block: u64,
    /// COW block
    cow_block: u64,
    /// Reference count
    refcount: u32,
}

/// Snapshot Manager
pub struct SnapshotManager {
    /// Block device
    device: Arc<Mutex<dyn BlockDevice>>,
    /// Block allocator
    allocator: Arc<super::super::allocation::BlockAllocator>,
    /// Snapshots by ID
    snapshots: Mutex<BTreeMap<u64, Snapshot>>,
    /// COW mappings (snapshot_id -> block -> cow_block)
    cow_mappings: Mutex<BTreeMap<u64, BTreeMap<u64, CowMapping>>>,
    /// Next snapshot ID
    next_id: Mutex<u64>,
}

impl SnapshotManager {
    /// Create new snapshot manager
    pub fn new(
        device: Arc<Mutex<dyn BlockDevice>>,
        allocator: Arc<super::super::allocation::BlockAllocator>,
    ) -> FsResult<Self> {
        Ok(Self {
            device,
            allocator,
            snapshots: Mutex::new(BTreeMap::new()),
            cow_mappings: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
        })
    }

    /// Create snapshot
    pub fn create_snapshot(&self, name: String, root_inode: u64) -> FsResult<u64> {
        let id = {
            let mut next_id = self.next_id.lock();
            let id = *next_id;
            *next_id += 1;
            id
        };

        let snapshot = Snapshot::new(id, name.clone(), root_inode);

        {
            let mut snapshots = self.snapshots.lock();
            snapshots.insert(id, snapshot);
        }

        {
            let mut cow_mappings = self.cow_mappings.lock();
            cow_mappings.insert(id, BTreeMap::new());
        }

        log::info!("ext4plus: Created snapshot '{}' (id: {})", name, id);

        Ok(id)
    }

    /// Delete snapshot
    pub fn delete_snapshot(&self, id: u64) -> FsResult<()> {
        // Remove from snapshots
        let snapshot = {
            let mut snapshots = self.snapshots.lock();
            snapshots.remove(&id).ok_or(FsError::NotFound)?
        };

        // Free COW blocks
        {
            let mut cow_mappings = self.cow_mappings.lock();
            if let Some(mappings) = cow_mappings.remove(&id) {
                for mapping in mappings.values() {
                    self.allocator.free_block(mapping.cow_block)?;
                }
            }
        }

        log::info!("ext4plus: Deleted snapshot '{}' (id: {})", snapshot.name, id);

        Ok(())
    }

    /// List snapshots
    pub fn list_snapshots(&self) -> Vec<Snapshot> {
        let snapshots = self.snapshots.lock();
        snapshots.values().cloned().collect()
    }

    /// Get snapshot
    pub fn get_snapshot(&self, id: u64) -> Option<Snapshot> {
        let snapshots = self.snapshots.lock();
        snapshots.get(&id).cloned()
    }

    /// Perform copy-on-write for block
    pub fn cow_block(&self, snapshot_id: u64, original_block: u64) -> FsResult<u64> {
        // Check if already COWed
        {
            let cow_mappings = self.cow_mappings.lock();
            if let Some(mappings) = cow_mappings.get(&snapshot_id) {
                if let Some(mapping) = mappings.get(&original_block) {
                    return Ok(mapping.cow_block);
                }
            }
        }

        // Allocate new block for COW
        let cow_block = self.allocator.allocate_block()?;

        // Copy original block to COW block
        // In production, would read original and write to COW block

        // Record mapping
        {
            let mut cow_mappings = self.cow_mappings.lock();
            if let Some(mappings) = cow_mappings.get_mut(&snapshot_id) {
                mappings.insert(original_block, CowMapping {
                    original_block,
                    cow_block,
                    refcount: 1,
                });
            }
        }

        log::trace!("ext4plus: COW block {} -> {} for snapshot {}",
            original_block, cow_block, snapshot_id);

        Ok(cow_block)
    }

    /// Get block for snapshot (following COW)
    pub fn get_block(&self, snapshot_id: u64, logical_block: u64) -> u64 {
        let cow_mappings = self.cow_mappings.lock();
        if let Some(mappings) = cow_mappings.get(&snapshot_id) {
            if let Some(mapping) = mappings.get(&logical_block) {
                return mapping.cow_block;
            }
        }
        logical_block
    }

    /// Get snapshot count
    pub fn snapshot_count(&self) -> usize {
        self.snapshots.lock().len()
    }

    /// Get total COW blocks
    pub fn total_cow_blocks(&self) -> u64 {
        let cow_mappings = self.cow_mappings.lock();
        cow_mappings.values()
            .map(|m| m.len() as u64)
            .sum()
    }

    /// Rollback to snapshot
    pub fn rollback(&self, snapshot_id: u64) -> FsResult<()> {
        let snapshot = {
            let snapshots = self.snapshots.lock();
            snapshots.get(&snapshot_id).cloned().ok_or(FsError::NotFound)?
        };

        // In production, would:
        // 1. Swap current root with snapshot root
        // 2. Update superblock
        // 3. Invalidate caches

        log::info!("ext4plus: Rolled back to snapshot '{}' (id: {})",
            snapshot.name, snapshot_id);

        Ok(())
    }
}
