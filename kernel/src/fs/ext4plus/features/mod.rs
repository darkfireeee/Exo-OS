//! Advanced Features
//!
//! Modern filesystem features:
//! - Snapshots (copy-on-write)
//! - Compression (LZ4, Zstandard)
//! - Encryption (AES-256-XTS)
//! - Deduplication (block-level)

pub mod snapshot;
pub mod compression;
pub mod encryption;
pub mod dedup;

use crate::fs::{FsError, FsResult};
use crate::fs::block::BlockDevice;
use alloc::sync::Arc;
use spin::Mutex;

pub use snapshot::{SnapshotManager, Snapshot};
pub use compression::{CompressionManager, CompressionAlgorithm};
pub use encryption::{EncryptionManager, EncryptionAlgorithm};
pub use dedup::{DeduplicationManager, DedupStats};

/// Feature Manager
///
/// Coordinates all advanced features
pub struct FeatureManager {
    /// Snapshot manager
    snapshot_manager: Arc<SnapshotManager>,
    /// Compression manager
    compression_manager: Arc<CompressionManager>,
    /// Encryption manager
    encryption_manager: Arc<EncryptionManager>,
    /// Deduplication manager
    dedup_manager: Arc<DeduplicationManager>,
}

impl FeatureManager {
    /// Create new feature manager
    pub fn new(
        device: Arc<Mutex<dyn BlockDevice>>,
        allocator: Arc<super::allocation::BlockAllocator>,
        inode_manager: Arc<super::inode::InodeManager>,
    ) -> FsResult<Arc<Self>> {
        let snapshot_manager = Arc::new(SnapshotManager::new(
            Arc::clone(&device),
            Arc::clone(&allocator),
        )?);

        let compression_manager = Arc::new(CompressionManager::new());

        let encryption_manager = Arc::new(EncryptionManager::new());

        let dedup_manager = Arc::new(DeduplicationManager::new(
            Arc::clone(&allocator),
        ));

        Ok(Arc::new(Self {
            snapshot_manager,
            compression_manager,
            encryption_manager,
            dedup_manager,
        }))
    }

    /// Get snapshot manager
    pub fn snapshot_manager(&self) -> &Arc<SnapshotManager> {
        &self.snapshot_manager
    }

    /// Get compression manager
    pub fn compression_manager(&self) -> &Arc<CompressionManager> {
        &self.compression_manager
    }

    /// Get encryption manager
    pub fn encryption_manager(&self) -> &Arc<EncryptionManager> {
        &self.encryption_manager
    }

    /// Get deduplication manager
    pub fn dedup_manager(&self) -> &Arc<DeduplicationManager> {
        &self.dedup_manager
    }

    /// Get feature statistics
    pub fn stats(&self) -> FeatureStats {
        FeatureStats {
            snapshots: self.snapshot_manager.snapshot_count(),
            compression_ratio: self.compression_manager.compression_ratio(),
            encrypted_blocks: self.encryption_manager.encrypted_block_count(),
            dedup_stats: self.dedup_manager.stats(),
        }
    }
}

/// Feature statistics
#[derive(Debug, Clone)]
pub struct FeatureStats {
    pub snapshots: usize,
    pub compression_ratio: f64,
    pub encrypted_blocks: u64,
    pub dedup_stats: super::features::dedup::DedupStatsSnapshot,
}
