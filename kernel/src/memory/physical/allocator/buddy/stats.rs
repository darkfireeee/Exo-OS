//! # memory/physical/allocator/buddy/stats.rs — Statistics and fragmentation metrics
//!
//! Fragmentation state analysis and free memory ratio calculation.

use crate::memory::core::{ZoneType, BUDDY_ORDER_COUNT};

/// Statistics of a buddy zone.
#[derive(Copy, Clone, Debug)]
pub struct BuddyZoneStats {
    pub zone_type: ZoneType,
    pub numa_node: u8,
    pub total_frames: usize,
    pub free_frames: usize,
    pub alloc_count: usize,
    pub free_count: usize,
    pub fail_count: usize,
    pub split_count: usize,
    pub merge_count: usize,
    /// Number of free blocks per order (0..BUDDY_ORDER_COUNT).
    pub free_by_order: [usize; BUDDY_ORDER_COUNT],
}

impl BuddyZoneStats {
    /// Free memory percentage (0.0-100.0).
    pub fn free_percent(&self) -> f32 {
        if self.total_frames == 0 {
            return 0.0;
        }
        self.free_frames as f32 * 100.0 / self.total_frames as f32
    }

    /// Fragmentation ratio (0.0 = none, 1.0 = maximum).
    /// Measures the gap between total free memory and large-order blocks.
    pub fn fragmentation_ratio(&self) -> f32 {
        if self.free_frames == 0 {
            return 0.0;
        }
        let largest_order = (0..BUDDY_ORDER_COUNT)
            .rev()
            .find(|&o| self.free_by_order[o] > 0)
            .unwrap_or(0);
        let max_contiguous = self.free_by_order[largest_order] * (1 << largest_order);
        1.0 - (max_contiguous as f32 / self.free_frames as f32)
    }
}
