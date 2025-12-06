//! ext4 Block Allocator

use super::Ext4GroupDesc;

/// Block Allocator
pub struct BlockAllocator {
    /// Total blocks
    total_blocks: u32,
    
    /// Next free hint
    next_free: u32,
}

impl BlockAllocator {
    pub fn new(total_blocks: u32, _group_descriptors: &[Ext4GroupDesc]) -> Self {
        Self {
            total_blocks,
            next_free: 0,
        }
    }
    
    /// Alloue un block
    pub fn allocate(&mut self) -> Option<u64> {
        if self.next_free < self.total_blocks {
            let block = self.next_free;
            self.next_free += 1;
            Some(block as u64)
        } else {
            None
        }
    }
}
