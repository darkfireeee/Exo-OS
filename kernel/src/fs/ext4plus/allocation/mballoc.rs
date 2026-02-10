//! Multi-Block Allocator (mballoc)
//!
//! Advanced multi-block allocator for efficient extent allocation.
//! Optimized for allocating large contiguous regions.

use super::BitmapAllocator;
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Allocation request
#[derive(Debug)]
pub struct AllocationRequest {
    /// Number of blocks requested
    pub count: u32,
    /// Goal block (hint for where to allocate)
    pub goal: Option<u64>,
    /// Minimum acceptable blocks
    pub min_count: u32,
    /// Flags
    pub flags: AllocationFlags,
}

/// Allocation flags
#[derive(Debug, Clone, Copy)]
pub struct AllocationFlags {
    /// Try to allocate near goal
    pub prefer_goal: bool,
    /// Allow partial allocation
    pub allow_partial: bool,
    /// Preallocation (for future use)
    pub preallocate: bool,
}

impl AllocationFlags {
    pub fn default() -> Self {
        Self {
            prefer_goal: true,
            allow_partial: false,
            preallocate: false,
        }
    }
}

/// Allocation result
#[derive(Debug)]
pub struct AllocationResult {
    /// Starting block
    pub start_block: u64,
    /// Number of blocks allocated
    pub count: u32,
}

/// Multi-Block Allocator
pub struct MultiBlockAllocator {
    /// Underlying bitmap allocator
    bitmap_allocator: Arc<BitmapAllocator>,
    /// Allocation statistics
    stats: MballocStats,
}

impl MultiBlockAllocator {
    /// Create new multi-block allocator
    pub fn new(bitmap_allocator: Arc<BitmapAllocator>) -> FsResult<Self> {
        Ok(Self {
            bitmap_allocator,
            stats: MballocStats::new(),
        })
    }

    /// Allocate blocks
    pub fn allocate(&self, count: u32) -> FsResult<u64> {
        let request = AllocationRequest {
            count,
            goal: None,
            min_count: count,
            flags: AllocationFlags::default(),
        };

        let result = self.allocate_with_request(&request)?;

        self.stats.allocations.fetch_add(1, Ordering::Relaxed);
        self.stats.blocks_allocated.fetch_add(result.count as u64, Ordering::Relaxed);

        if result.count >= 8 {
            self.stats.large_allocations.fetch_add(1, Ordering::Relaxed);
        }

        Ok(result.start_block)
    }

    /// Allocate with detailed request
    pub fn allocate_with_request(&self, request: &AllocationRequest) -> FsResult<AllocationResult> {
        // Strategy 1: Try to allocate near goal
        if let Some(goal) = request.goal {
            if request.flags.prefer_goal {
                if let Ok(block) = self.allocate_near_goal(goal, request.count) {
                    return Ok(AllocationResult {
                        start_block: block,
                        count: request.count,
                    });
                }
            }
        }

        // Strategy 2: Try buddy allocation for power-of-2 sizes
        if request.count.is_power_of_two() {
            if let Ok(block) = self.buddy_allocate(request.count) {
                return Ok(AllocationResult {
                    start_block: block,
                    count: request.count,
                });
            }
        }

        // Strategy 3: Linear search for contiguous blocks
        if let Ok(block) = self.bitmap_allocator.allocate_blocks(request.count) {
            return Ok(AllocationResult {
                start_block: block,
                count: request.count,
            });
        }

        // Strategy 4: Partial allocation (if allowed)
        if request.flags.allow_partial && request.min_count < request.count {
            for count in (request.min_count..request.count).rev() {
                if let Ok(block) = self.bitmap_allocator.allocate_blocks(count) {
                    return Ok(AllocationResult {
                        start_block: block,
                        count,
                    });
                }
            }
        }

        Err(FsError::NoSpace)
    }

    /// Allocate near goal block
    fn allocate_near_goal(&self, goal: u64, count: u32) -> FsResult<u64> {
        let blocks_per_group = self.bitmap_allocator.blocks_per_group();
        let goal_group = goal / blocks_per_group as u64;

        // Try goal group first
        // In production, would have direct access to group bitmaps

        // For now, use standard allocation
        self.bitmap_allocator.allocate_blocks(count)
    }

    /// Buddy allocator
    fn buddy_allocate(&self, count: u32) -> FsResult<u64> {
        // Simplified buddy allocation
        // In production, would maintain buddy trees for each group

        self.bitmap_allocator.allocate_blocks(count)
    }

    /// Allocate with alignment
    pub fn allocate_aligned(&self, count: u32, alignment: u32) -> FsResult<u64> {
        // In production, would ensure allocated blocks are aligned
        // For now, use standard allocation

        self.allocate(count)
    }

    /// Get statistics
    pub fn stats(&self) -> MballocStatsSnapshot {
        MballocStatsSnapshot {
            allocations: self.stats.allocations.load(Ordering::Relaxed),
            blocks_allocated: self.stats.blocks_allocated.load(Ordering::Relaxed),
            large_allocations: self.stats.large_allocations.load(Ordering::Relaxed),
        }
    }
}

/// Mballoc statistics
struct MballocStats {
    allocations: AtomicU64,
    blocks_allocated: AtomicU64,
    large_allocations: AtomicU64,
}

impl MballocStats {
    fn new() -> Self {
        Self {
            allocations: AtomicU64::new(0),
            blocks_allocated: AtomicU64::new(0),
            large_allocations: AtomicU64::new(0),
        }
    }
}

/// Statistics snapshot
#[derive(Debug, Clone, Copy)]
pub struct MballocStatsSnapshot {
    pub allocations: u64,
    pub blocks_allocated: u64,
    pub large_allocations: u64,
}
