//! Block Allocation
//!
//! Advanced block allocation strategies:
//! - Bitmap-based allocation (balloc)
//! - Multi-block allocator (mballoc)
//! - Preallocation
//! - AI-guided allocation
//! - Defragmentation

pub mod balloc;
pub mod mballoc;
pub mod prealloc;
pub mod ai_allocator;
pub mod defrag;

use crate::fs::{FsError, FsResult};
use crate::fs::block::BlockDevice;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

pub use balloc::BitmapAllocator;
pub use mballoc::MultiBlockAllocator;
pub use prealloc::PreallocManager;
pub use ai_allocator::AiAllocator;
pub use defrag::Defragmenter;

/// Block Allocator
///
/// Main block allocator with multiple strategies
pub struct BlockAllocator {
    /// Block device
    device: Arc<Mutex<dyn BlockDevice>>,
    /// Bitmap allocator (low-level)
    bitmap_allocator: Arc<BitmapAllocator>,
    /// Multi-block allocator (extents)
    mballoc: Arc<MultiBlockAllocator>,
    /// Preallocation manager
    prealloc: Arc<PreallocManager>,
    /// AI-guided allocator
    ai_allocator: Arc<AiAllocator>,
    /// Defragmenter
    defragmenter: Arc<Defragmenter>,
    /// Total free blocks
    total_free: AtomicU64,
    /// Statistics
    stats: AllocationStats,
}

impl BlockAllocator {
    /// Create new block allocator
    pub fn new(
        device: Arc<Mutex<dyn BlockDevice>>,
        superblock: &super::superblock::Ext4plusSuperblock,
        group_desc_table: &super::group_desc::GroupDescriptorTable,
    ) -> FsResult<Self> {
        log::info!("ext4plus: Initializing block allocator");

        let bitmap_allocator = Arc::new(BitmapAllocator::new(
            Arc::clone(&device),
            superblock,
            group_desc_table,
        )?);

        let mballoc = Arc::new(MultiBlockAllocator::new(Arc::clone(&bitmap_allocator))?);

        let prealloc = Arc::new(PreallocManager::new());

        let ai_allocator = Arc::new(AiAllocator::new(Arc::clone(&bitmap_allocator)));

        let defragmenter = Arc::new(Defragmenter::new(Arc::clone(&bitmap_allocator)));

        let total_free = superblock.free_blocks();

        log::info!("ext4plus: Block allocator initialized");
        log::info!("  Total free blocks: {} ({} MB)",
            total_free,
            total_free * superblock.block_size() as u64 / 1024 / 1024
        );

        Ok(Self {
            device,
            bitmap_allocator,
            mballoc,
            prealloc,
            ai_allocator,
            defragmenter,
            total_free: AtomicU64::new(total_free),
            stats: AllocationStats::new(),
        })
    }

    /// Allocate single block
    pub fn allocate_block(&self) -> FsResult<u64> {
        self.allocate_blocks(1)
    }

    /// Allocate N contiguous blocks
    pub fn allocate_blocks(&self, count: u32) -> FsResult<u64> {
        if count == 0 {
            return Err(FsError::InvalidArgument);
        }

        // Check free space
        if (count as u64) > self.total_free.load(Ordering::Relaxed) {
            return Err(FsError::NoSpace);
        }

        // Use multi-block allocator for contiguous allocations
        let block = if count > 1 {
            self.mballoc.allocate(count)?
        } else {
            // Use AI-guided single block allocation
            self.ai_allocator.allocate_single()?
        };

        // Update free count
        self.total_free.fetch_sub(count as u64, Ordering::Relaxed);

        // Update stats
        self.stats.allocations.fetch_add(1, Ordering::Relaxed);
        self.stats.blocks_allocated.fetch_add(count as u64, Ordering::Relaxed);

        log::trace!("ext4plus: Allocated {} blocks starting at {}", count, block);

        Ok(block)
    }

    /// Free single block
    pub fn free_block(&self, block: u64) -> FsResult<()> {
        self.free_blocks(block, 1)
    }

    /// Free N contiguous blocks
    pub fn free_blocks(&self, start_block: u64, count: u32) -> FsResult<()> {
        if count == 0 {
            return Ok(());
        }

        self.bitmap_allocator.free_blocks(start_block, count)?;

        // Update free count
        self.total_free.fetch_add(count as u64, Ordering::Relaxed);

        // Update stats
        self.stats.frees.fetch_add(1, Ordering::Relaxed);
        self.stats.blocks_freed.fetch_add(count as u64, Ordering::Relaxed);

        log::trace!("ext4plus: Freed {} blocks starting at {}", count, start_block);

        Ok(())
    }

    /// Preallocate blocks for inode
    pub fn preallocate(&self, inode: u64, count: u32) -> FsResult<Vec<u64>> {
        self.prealloc.preallocate(inode, count, &self.mballoc)
    }

    /// Get preallocated block for inode
    pub fn get_preallocated(&self, inode: u64) -> Option<u64> {
        self.prealloc.get_block(inode)
    }

    /// Free preallocated blocks for inode
    pub fn free_preallocated(&self, inode: u64) -> FsResult<()> {
        let blocks = self.prealloc.free_preallocated(inode);
        for block in blocks {
            self.free_block(block)?;
        }
        Ok(())
    }

    /// Run defragmentation
    pub fn defragment(&self) -> FsResult<DefragStats> {
        self.defragmenter.defragment()
    }

    /// Get free blocks count
    pub fn free_blocks_count(&self) -> u64 {
        self.total_free.load(Ordering::Relaxed)
    }

    /// Get allocator statistics
    pub fn stats(&self) -> AllocatorStats {
        AllocatorStats {
            total_free_blocks: self.total_free.load(Ordering::Relaxed),
            total_allocations: self.stats.allocations.load(Ordering::Relaxed),
            total_frees: self.stats.frees.load(Ordering::Relaxed),
            blocks_allocated: self.stats.blocks_allocated.load(Ordering::Relaxed),
            blocks_freed: self.stats.blocks_freed.load(Ordering::Relaxed),
            fragmentation_percent: self.bitmap_allocator.calculate_fragmentation(),
        }
    }

    /// Sync all dirty bitmaps
    pub fn sync(&self) -> FsResult<()> {
        self.bitmap_allocator.sync()
    }
}

/// Allocation statistics
struct AllocationStats {
    allocations: AtomicU64,
    frees: AtomicU64,
    blocks_allocated: AtomicU64,
    blocks_freed: AtomicU64,
}

impl AllocationStats {
    fn new() -> Self {
        Self {
            allocations: AtomicU64::new(0),
            frees: AtomicU64::new(0),
            blocks_allocated: AtomicU64::new(0),
            blocks_freed: AtomicU64::new(0),
        }
    }
}

/// Allocator statistics (public)
#[derive(Debug, Clone, Copy)]
pub struct AllocatorStats {
    pub total_free_blocks: u64,
    pub total_allocations: u64,
    pub total_frees: u64,
    pub blocks_allocated: u64,
    pub blocks_freed: u64,
    pub fragmentation_percent: f64,
}

/// Defragmentation statistics
#[derive(Debug, Clone, Copy)]
pub struct DefragStats {
    pub blocks_moved: u64,
    pub extents_merged: u64,
    pub fragmentation_before: f64,
    pub fragmentation_after: f64,
}
