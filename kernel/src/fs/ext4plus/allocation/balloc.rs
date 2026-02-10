//! Bitmap Block Allocator
//!
//! Low-level block allocation using bitmaps.
//! Each group has a bitmap tracking free/allocated blocks.

use crate::fs::{FsError, FsResult};
use crate::fs::block::BlockDevice;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

/// Block group
struct BlockGroup {
    /// Group number
    group_num: u32,
    /// Block bitmap (1 bit per block)
    bitmap: Vec<u8>,
    /// Free blocks count
    free_blocks: AtomicU32,
    /// Dirty flag
    dirty: bool,
}

impl BlockGroup {
    /// Create new block group
    fn new(group_num: u32, blocks_per_group: u32) -> Self {
        let bitmap_size = (blocks_per_group + 7) / 8;
        Self {
            group_num,
            bitmap: alloc::vec![0xFF; bitmap_size as usize], // All free
            free_blocks: AtomicU32::new(blocks_per_group),
            dirty: false,
        }
    }

    /// Check if block is free
    fn is_free(&self, block: u32) -> bool {
        let byte_idx = (block / 8) as usize;
        let bit_idx = block % 8;
        if byte_idx >= self.bitmap.len() {
            return false;
        }
        (self.bitmap[byte_idx] & (1 << bit_idx)) != 0
    }

    /// Allocate block
    fn allocate(&mut self, block: u32) {
        let byte_idx = (block / 8) as usize;
        let bit_idx = block % 8;
        if byte_idx < self.bitmap.len() {
            self.bitmap[byte_idx] &= !(1 << bit_idx);
            self.free_blocks.fetch_sub(1, Ordering::Relaxed);
            self.dirty = true;
        }
    }

    /// Free block
    fn free(&mut self, block: u32) {
        let byte_idx = (block / 8) as usize;
        let bit_idx = block % 8;
        if byte_idx < self.bitmap.len() {
            if (self.bitmap[byte_idx] & (1 << bit_idx)) == 0 {
                self.bitmap[byte_idx] |= 1 << bit_idx;
                self.free_blocks.fetch_add(1, Ordering::Relaxed);
                self.dirty = true;
            }
        }
    }

    /// Find first free block
    fn find_free_block(&self) -> Option<u32> {
        for (byte_idx, &byte) in self.bitmap.iter().enumerate() {
            if byte != 0 {
                for bit_idx in 0..8 {
                    if (byte & (1 << bit_idx)) != 0 {
                        return Some((byte_idx * 8 + bit_idx) as u32);
                    }
                }
            }
        }
        None
    }

    /// Find N contiguous free blocks
    fn find_contiguous(&self, count: u32) -> Option<u32> {
        let mut run_start = None;
        let mut run_length = 0;

        for block in 0..(self.bitmap.len() * 8) as u32 {
            if self.is_free(block) {
                if run_start.is_none() {
                    run_start = Some(block);
                    run_length = 1;
                } else {
                    run_length += 1;
                    if run_length >= count {
                        return run_start;
                    }
                }
            } else {
                run_start = None;
                run_length = 0;
            }
        }

        None
    }

    /// Get free blocks count
    fn free_count(&self) -> u32 {
        self.free_blocks.load(Ordering::Relaxed)
    }
}

/// Bitmap Allocator
pub struct BitmapAllocator {
    /// Block device
    device: Arc<Mutex<dyn BlockDevice>>,
    /// Block size
    block_size: usize,
    /// Blocks per group
    blocks_per_group: u32,
    /// Block groups
    groups: Vec<Mutex<BlockGroup>>,
}

impl BitmapAllocator {
    /// Create new bitmap allocator
    pub fn new(
        device: Arc<Mutex<dyn BlockDevice>>,
        superblock: &super::super::superblock::Ext4plusSuperblock,
        group_desc_table: &super::super::group_desc::GroupDescriptorTable,
    ) -> FsResult<Self> {
        let block_size = superblock.block_size();
        let blocks_per_group = superblock.s_blocks_per_group;
        let groups_count = group_desc_table.count();

        log::debug!("ext4plus BitmapAllocator: Initializing {} block groups", groups_count);

        let mut groups = Vec::new();
        for i in 0..groups_count {
            groups.push(Mutex::new(BlockGroup::new(i, blocks_per_group)));
        }

        Ok(Self {
            device,
            block_size,
            blocks_per_group,
            groups,
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

        // Try each group
        for (group_idx, group_mutex) in self.groups.iter().enumerate() {
            let mut group = group_mutex.lock();

            if group.free_count() < count {
                continue;
            }

            // For single block, use fast path
            if count == 1 {
                if let Some(block) = group.find_free_block() {
                    group.allocate(block);
                    let global_block = (group_idx as u32 * self.blocks_per_group) + block;
                    return Ok(global_block as u64);
                }
            } else {
                // Multi-block allocation
                if let Some(start_block) = group.find_contiguous(count) {
                    for i in 0..count {
                        group.allocate(start_block + i);
                    }
                    let global_block = (group_idx as u32 * self.blocks_per_group) + start_block;
                    return Ok(global_block as u64);
                }
            }
        }

        Err(FsError::NoSpace)
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

        let group_idx = (start_block / self.blocks_per_group as u64) as usize;
        let block_in_group = (start_block % self.blocks_per_group as u64) as u32;

        if group_idx >= self.groups.len() {
            return Err(FsError::InvalidArgument);
        }

        let mut group = self.groups[group_idx].lock();
        for i in 0..count {
            group.free(block_in_group + i);
        }

        Ok(())
    }

    /// Calculate fragmentation percentage
    pub fn calculate_fragmentation(&self) -> f64 {
        let mut total_gaps = 0u64;
        let mut total_free = 0u64;

        for group_mutex in &self.groups {
            let group = group_mutex.lock();
            total_free += group.free_count() as u64;

            // Count gaps (transitions from free to allocated)
            let mut in_free_run = false;
            for byte in &group.bitmap {
                for bit in 0..8 {
                    let is_free = (byte & (1 << bit)) != 0;
                    if is_free && !in_free_run {
                        total_gaps += 1;
                        in_free_run = true;
                    } else if !is_free {
                        in_free_run = false;
                    }
                }
            }
        }

        if total_free == 0 {
            return 0.0;
        }

        (total_gaps as f64 / total_free as f64) * 100.0
    }

    /// Sync dirty bitmaps
    pub fn sync(&self) -> FsResult<()> {
        for group_mutex in &self.groups {
            let mut group = group_mutex.lock();
            if group.dirty {
                // In production, would write bitmap to disk
                log::trace!("ext4plus: Syncing dirty bitmap for group {}", group.group_num);
                group.dirty = false;
            }
        }
        Ok(())
    }

    /// Get total free blocks
    pub fn total_free(&self) -> u64 {
        self.groups.iter()
            .map(|g| g.lock().free_count() as u64)
            .sum()
    }

    /// Get group count
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Get blocks per group
    pub fn blocks_per_group(&self) -> u32 {
        self.blocks_per_group
    }
}
