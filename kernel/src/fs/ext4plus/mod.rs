//! ext4plus - Enhanced ext4 Filesystem Implementation
//!
//! Production-quality ext4 implementation with modern features:
//! - Full ext4 compatibility
//! - Extent-based block mapping
//! - Directory indexing (HTree)
//! - Advanced allocation (multi-block allocator, preallocation)
//! - AI-guided allocation hints
//! - Snapshots, compression, encryption, deduplication
//! - Integrated with cache, integrity, and I/O subsystems
//!
//! # Architecture
//! - `superblock`: Superblock management
//! - `group_desc`: Block group descriptors
//! - `inode`: Inode structures and operations
//! - `directory`: Directory management (linear and HTree)
//! - `allocation`: Block allocation strategies
//! - `features`: Advanced features (snapshots, compression, encryption, dedup)
//!
//! # Performance
//! - O(1) block allocation via buddy allocator
//! - O(log n) file block lookup via extents
//! - O(1) average directory lookup via HTree
//! - Prefetching and intelligent caching
//! - AI-guided placement for optimal performance

use crate::fs::{FsError, FsResult};
use crate::fs::block::BlockDevice;
use alloc::sync::Arc;
use spin::Mutex;

pub mod superblock;
pub mod group_desc;
pub mod inode;
pub mod directory;
pub mod allocation;
pub mod features;

pub use superblock::{Ext4plusSuperblock, EXT4PLUS_SUPER_MAGIC};
pub use group_desc::{GroupDescriptor, GroupDescriptorTable};
pub use inode::Ext4plusInode;

/// ext4plus constants
pub const BLOCK_SIZE: usize = 4096;
pub const INODE_SIZE: usize = 256;
pub const BLOCKS_PER_GROUP: u32 = 32768;
pub const INODES_PER_GROUP: u32 = 8192;

/// ext4plus Filesystem
///
/// Complete ext4plus filesystem instance with all subsystems
pub struct Ext4plusFs {
    /// Block device
    device: Arc<Mutex<dyn BlockDevice>>,
    /// Superblock
    superblock: Ext4plusSuperblock,
    /// Block group descriptor table
    group_desc_table: GroupDescriptorTable,
    /// Block allocator
    allocator: Arc<allocation::BlockAllocator>,
    /// Inode manager
    inode_manager: Arc<inode::InodeManager>,
    /// Directory manager
    dir_manager: Arc<directory::DirectoryManager>,
    /// Feature manager
    feature_manager: Arc<features::FeatureManager>,
}

impl Ext4plusFs {
    /// Mount ext4plus filesystem
    pub fn mount(device: Arc<Mutex<dyn BlockDevice>>) -> FsResult<Self> {
        log::info!("ext4plus: Mounting filesystem");

        // Read and validate superblock
        let superblock = Ext4plusSuperblock::read(&device)?;
        superblock.validate()?;

        log::info!("ext4plus: Superblock validated");
        log::info!("  Block size: {} bytes", superblock.block_size());
        log::info!("  Total blocks: {}", superblock.total_blocks());
        log::info!("  Free blocks: {}", superblock.free_blocks());
        log::info!("  Total inodes: {}", superblock.s_inodes_count);
        log::info!("  Free inodes: {}", superblock.s_free_inodes_count);

        // Read group descriptor table
        let group_desc_table = GroupDescriptorTable::read(&device, &superblock)?;
        log::info!("ext4plus: Loaded {} block groups", group_desc_table.count());

        // Initialize subsystems
        let allocator = Arc::new(allocation::BlockAllocator::new(
            Arc::clone(&device),
            &superblock,
            &group_desc_table,
        )?);
        log::info!("ext4plus: Block allocator initialized");

        let inode_manager = inode::InodeManager::new(
            Arc::clone(&device),
            superblock.clone(),
            Arc::clone(&allocator),
        )?;
        log::info!("ext4plus: Inode manager initialized");

        let dir_manager = directory::DirectoryManager::new(
            Arc::clone(&device),
            Arc::clone(&inode_manager),
        )?;
        log::info!("ext4plus: Directory manager initialized");

        let feature_manager = features::FeatureManager::new(
            Arc::clone(&device),
            Arc::clone(&allocator),
            Arc::clone(&inode_manager),
        )?;
        log::info!("ext4plus: Feature manager initialized");

        log::info!("ext4plus: Mount successful");

        Ok(Self {
            device,
            superblock,
            group_desc_table,
            allocator,
            inode_manager,
            dir_manager,
            feature_manager,
        })
    }

    /// Unmount filesystem (sync and cleanup)
    pub fn unmount(&mut self) -> FsResult<()> {
        log::info!("ext4plus: Unmounting filesystem");

        // Sync all caches
        self.inode_manager.sync_all()?;
        self.allocator.sync()?;

        // Write superblock
        self.superblock.write(&self.device)?;

        // Write group descriptors
        self.group_desc_table.write(&self.device, &self.superblock)?;

        log::info!("ext4plus: Unmount successful");
        Ok(())
    }

    /// Get filesystem statistics
    pub fn stats(&self) -> Ext4plusStats {
        Ext4plusStats {
            total_blocks: self.superblock.total_blocks(),
            free_blocks: self.allocator.free_blocks_count(),
            used_blocks: self.superblock.total_blocks() - self.allocator.free_blocks_count(),
            total_inodes: self.superblock.s_inodes_count as u64,
            free_inodes: self.superblock.s_free_inodes_count as u64,
            used_inodes: (self.superblock.s_inodes_count - self.superblock.s_free_inodes_count) as u64,
            block_size: self.superblock.block_size() as u64,
            allocator_stats: self.allocator.stats(),
        }
    }

    /// Get superblock
    pub fn superblock(&self) -> &Ext4plusSuperblock {
        &self.superblock
    }

    /// Get allocator
    pub fn allocator(&self) -> &Arc<allocation::BlockAllocator> {
        &self.allocator
    }

    /// Get inode manager
    pub fn inode_manager(&self) -> &Arc<inode::InodeManager> {
        &self.inode_manager
    }

    /// Get directory manager
    pub fn dir_manager(&self) -> &Arc<directory::DirectoryManager> {
        &self.dir_manager
    }

    /// Get feature manager
    pub fn feature_manager(&self) -> &Arc<features::FeatureManager> {
        &self.feature_manager
    }
}

/// Filesystem statistics
#[derive(Debug, Clone, Copy)]
pub struct Ext4plusStats {
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub used_blocks: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub used_inodes: u64,
    pub block_size: u64,
    pub allocator_stats: allocation::AllocatorStats,
}

/// Initialize ext4plus filesystem subsystem
pub fn init() {
    log::debug!("ext4plus filesystem subsystem initialized");
}
