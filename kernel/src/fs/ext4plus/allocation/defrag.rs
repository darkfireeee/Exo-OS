//! Defragmenter
//!
//! Online and offline defragmentation to reduce filesystem fragmentation.
//! Can run in background or on-demand.

use super::{BitmapAllocator, DefragStats};
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// Fragmented extent
#[derive(Debug, Clone)]
struct FragmentedExtent {
    /// Starting block
    start_block: u64,
    /// Block count
    count: u32,
    /// Inode (if known)
    inode: Option<u64>,
}

/// Defragmentation mode
#[derive(Debug, Clone, Copy)]
pub enum DefragMode {
    /// Quick pass (low impact)
    Quick,
    /// Normal defragmentation
    Normal,
    /// Aggressive defragmentation
    Aggressive,
}

/// Defragmenter
pub struct Defragmenter {
    /// Allocator
    bitmap_allocator: Arc<BitmapAllocator>,
    /// Running flag
    running: Mutex<bool>,
}

impl Defragmenter {
    /// Create new defragmenter
    pub fn new(bitmap_allocator: Arc<BitmapAllocator>) -> Self {
        Self {
            bitmap_allocator,
            running: Mutex::new(false),
        }
    }

    /// Run defragmentation
    pub fn defragment(&self) -> FsResult<DefragStats> {
        self.defragment_with_mode(DefragMode::Normal)
    }

    /// Run defragmentation with specific mode
    pub fn defragment_with_mode(&self, mode: DefragMode) -> FsResult<DefragStats> {
        // Check if already running
        {
            let mut running = self.running.lock();
            if *running {
                return Err(FsError::Again);
            }
            *running = true;
        }

        log::info!("ext4plus: Starting defragmentation ({:?} mode)", mode);

        let fragmentation_before = self.bitmap_allocator.calculate_fragmentation();

        // Defragmentation steps
        let mut blocks_moved = 0u64;
        let mut extents_merged = 0u64;

        // Step 1: Identify fragmented extents
        let fragments = self.identify_fragments();
        log::debug!("ext4plus: Found {} fragmented extents", fragments.len());

        // Step 2: Try to merge adjacent extents
        extents_merged += self.merge_extents(&fragments)?;

        // Step 3: Move blocks to create contiguous regions (if aggressive)
        if matches!(mode, DefragMode::Aggressive) {
            blocks_moved += self.relocate_blocks(&fragments)?;
        }

        let fragmentation_after = self.bitmap_allocator.calculate_fragmentation();

        // Clear running flag
        {
            let mut running = self.running.lock();
            *running = false;
        }

        let improvement = fragmentation_before - fragmentation_after;
        log::info!("ext4plus: Defragmentation complete");
        log::info!("  Blocks moved: {}", blocks_moved);
        log::info!("  Extents merged: {}", extents_merged);
        log::info!("  Fragmentation: {:.2}% -> {:.2}% (improvement: {:.2}%)",
            fragmentation_before, fragmentation_after, improvement);

        Ok(DefragStats {
            blocks_moved,
            extents_merged,
            fragmentation_before,
            fragmentation_after,
        })
    }

    /// Identify fragmented extents
    fn identify_fragments(&self) -> Vec<FragmentedExtent> {
        let mut fragments = Vec::new();

        // In production, would scan all inodes and their extent trees
        // For now, return empty list

        fragments
    }

    /// Merge adjacent extents
    fn merge_extents(&self, fragments: &[FragmentedExtent]) -> FsResult<u64> {
        let mut merged = 0u64;

        // In production, would:
        // 1. Find extents that can be merged (adjacent in file and on disk)
        // 2. Update inode extent trees to merge them
        // 3. This doesn't move data, just updates metadata

        log::debug!("ext4plus: Merged {} extents", merged);

        Ok(merged)
    }

    /// Relocate blocks to reduce fragmentation
    fn relocate_blocks(&self, fragments: &[FragmentedExtent]) -> FsResult<u64> {
        let mut moved = 0u64;

        // In production, would:
        // 1. Find scattered blocks
        // 2. Allocate contiguous region
        // 3. Copy blocks to new location
        // 4. Update extent trees
        // 5. Free old blocks

        log::debug!("ext4plus: Relocated {} blocks", moved);

        Ok(moved)
    }

    /// Check if defragmentation is running
    pub fn is_running(&self) -> bool {
        *self.running.lock()
    }

    /// Estimate defragmentation time
    pub fn estimate_time(&self, mode: DefragMode) -> u64 {
        let fragmentation = self.bitmap_allocator.calculate_fragmentation();

        // Simplified estimation (in seconds)
        let base_time = match mode {
            DefragMode::Quick => 10,
            DefragMode::Normal => 60,
            DefragMode::Aggressive => 300,
        };

        (base_time as f64 * (fragmentation / 100.0)) as u64
    }

    /// Get current fragmentation level
    pub fn fragmentation(&self) -> f64 {
        self.bitmap_allocator.calculate_fragmentation()
    }

    /// Check if defragmentation is needed
    pub fn needs_defrag(&self) -> bool {
        self.fragmentation() > 20.0 // More than 20% fragmentation
    }

    /// Get recommended mode
    pub fn recommended_mode(&self) -> DefragMode {
        let frag = self.fragmentation();
        if frag > 50.0 {
            DefragMode::Aggressive
        } else if frag > 20.0 {
            DefragMode::Normal
        } else {
            DefragMode::Quick
        }
    }
}
