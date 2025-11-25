//! Buddy Allocator - Physical Frame Allocation
//! 
//! Implements a buddy system allocator for physical frames with orders 0-12.
//! - Order 0 = 4KB (1 page)
//! - Order 12 = 16MB (4096 pages)
//! 
//! Features:
//! - Automatic coalescing on free
//! - First-fit allocation with order splitting
//! - Bitmap tracking for used/free frames
//! - Lock-free per-order free lists (future optimization)

use core::ptr::NonNull;
use core::mem::MaybeUninit;
use spin::Mutex;
use crate::memory::{MemoryError, MemoryResult};
use crate::memory::address::PhysicalAddress;

/// Maximum buddy order (12 = 16MB blocks)
pub const MAX_ORDER: usize = 12;

/// Number of orders (0-12 inclusive)
pub const NUM_ORDERS: usize = MAX_ORDER + 1;

/// Page size (4KB)
pub const PAGE_SIZE: usize = 4096;

/// Buddy block header stored at the beginning of each free block
#[repr(C)]
#[derive(Debug)]
struct BuddyBlock {
    next: Option<NonNull<BuddyBlock>>,
}

// Safety: BuddyBlock only contains a pointer which is safe to send between threads
// The actual safety depends on the allocator's lock
unsafe impl Send for BuddyBlock {}

impl BuddyBlock {
    fn new() -> Self {
        Self { next: None }
    }
}

/// Free list for a specific order
struct FreeList {
    head: Option<NonNull<BuddyBlock>>,
    count: usize,
}

// Safety: FreeList is protected by Mutex in global BUDDY, pointers are never accessed concurrently
unsafe impl Send for FreeList {}

impl FreeList {
    const fn new() -> Self {
        Self {
            head: None,
            count: 0,
        }
    }

    /// Add a block to the front of the free list
    unsafe fn push(&mut self, block: NonNull<BuddyBlock>) {
        let block_ptr = block.as_ptr();
        (*block_ptr).next = self.head;
        self.head = Some(block);
        self.count += 1;
    }

    /// Remove and return the first block
    fn pop(&mut self) -> Option<NonNull<BuddyBlock>> {
        if let Some(head) = self.head {
            unsafe {
                let head_ptr = head.as_ptr();
                self.head = (*head_ptr).next;
                self.count -= 1;
                Some(head)
            }
        } else {
            None
        }
    }

    /// Remove a specific block from the list (for coalescing)
    unsafe fn remove(&mut self, block: NonNull<BuddyBlock>) -> bool {
        let block_addr = block.as_ptr() as usize;
        
        // Check head
        if let Some(head) = self.head {
            if head.as_ptr() as usize == block_addr {
                self.head = (*head.as_ptr()).next;
                self.count -= 1;
                return true;
            }

            // Traverse list
            let mut current = head;
            loop {
                let current_ptr = current.as_ptr();
                if let Some(next) = (*current_ptr).next {
                    if next.as_ptr() as usize == block_addr {
                        (*current_ptr).next = (*next.as_ptr()).next;
                        self.count -= 1;
                        return true;
                    }
                    current = next;
                } else {
                    break;
                }
            }
        }
        false
    }

    fn is_empty(&self) -> bool {
        self.head.is_none()
    }
}

/// Bitmap for tracking used/free frames at the smallest granularity
struct Bitmap {
    data: &'static mut [u64],
    size_bits: usize,
}

impl Bitmap {
    /// Create bitmap from pre-allocated memory region
    unsafe fn new(addr: usize, size_bytes: usize, total_frames: usize) -> Self {
        let data = core::slice::from_raw_parts_mut(addr as *mut u64, size_bytes / 8);
        // Initialize all bits to 0 (free)
        for word in data.iter_mut() {
            *word = 0;
        }
        Self {
            data,
            size_bits: total_frames,
        }
    }

    /// Set bit (mark frame as used)
    fn set(&mut self, index: usize) {
        if index >= self.size_bits {
            return;
        }
        let word = index / 64;
        let bit = index % 64;
        self.data[word] |= 1u64 << bit;
    }

    /// Clear bit (mark frame as free)
    fn clear(&mut self, index: usize) {
        if index >= self.size_bits {
            return;
        }
        let word = index / 64;
        let bit = index % 64;
        self.data[word] &= !(1u64 << bit);
    }

    /// Test if bit is set
    fn test(&self, index: usize) -> bool {
        if index >= self.size_bits {
            return false;
        }
        let word = index / 64;
        let bit = index % 64;
        (self.data[word] & (1u64 << bit)) != 0
    }

    /// Mark a range of frames as used
    fn set_range(&mut self, start_frame: usize, count: usize) {
        for i in 0..count {
            self.set(start_frame + i);
        }
    }
}

/// Buddy System Allocator
pub struct BuddyAllocator {
    /// Free lists for each order (0-12)
    free_lists: [FreeList; NUM_ORDERS],
    
    /// Bitmap for tracking smallest frames (order 0)
    bitmap: Bitmap,
    
    /// Base physical address
    base_addr: PhysicalAddress,
    
    /// Total number of frames (4KB each)
    total_frames: usize,
    
    /// Statistics
    allocated_frames: usize,
    free_frames: usize,
}

impl BuddyAllocator {
    /// Create a new buddy allocator
    /// 
    /// # Safety
    /// - bitmap_addr must point to valid, writable memory of size bitmap_size
    /// - memory_start..memory_start+total_size must be valid physical memory
    pub unsafe fn new(
        bitmap_addr: usize,
        bitmap_size: usize,
        memory_start: PhysicalAddress,
        total_size: usize,
    ) -> Self {
        let total_frames = total_size / PAGE_SIZE;
        
        // Create uninitialized free lists
        let mut free_lists_uninit: [MaybeUninit<FreeList>; NUM_ORDERS] = 
            MaybeUninit::uninit().assume_init();
        
        for elem in &mut free_lists_uninit {
            elem.write(FreeList::new());
        }
        
        let free_lists = core::mem::transmute::<
            [MaybeUninit<FreeList>; NUM_ORDERS],
            [FreeList; NUM_ORDERS]
        >(free_lists_uninit);

        let bitmap = Bitmap::new(bitmap_addr, bitmap_size, total_frames);

        Self {
            free_lists,
            bitmap,
            base_addr: memory_start,
            total_frames,
            allocated_frames: 0,
            free_frames: 0,
        }
    }

    /// Add a free memory region to the allocator
    /// 
    /// This splits the region into buddy blocks and adds them to appropriate free lists
    pub fn add_region(&mut self, start: PhysicalAddress, size: usize) {
        let start_addr = start.value();
        let base_addr = self.base_addr.value();
        
        // Convert to frame index
        let start_frame = (start_addr - base_addr) / PAGE_SIZE;
        let num_frames = size / PAGE_SIZE;

        // Add blocks of largest possible order
        let mut current_frame = start_frame;
        let end_frame = start_frame + num_frames;

        while current_frame < end_frame {
            let remaining = end_frame - current_frame;
            
            // Find largest order that fits and is aligned
            let mut order = MAX_ORDER;
            loop {
                let block_size = 1 << order; // frames in this order
                if block_size <= remaining && (current_frame & (block_size - 1)) == 0 {
                    break;
                }
                if order == 0 {
                    break;
                }
                order -= 1;
            }

            // Add this block to free list
            let block_addr = base_addr + current_frame * PAGE_SIZE;
            unsafe {
                self.add_free_block(block_addr, order);
            }
            
            current_frame += 1 << order;
        }
    }

    /// Add a free block to the appropriate free list
    unsafe fn add_free_block(&mut self, addr: usize, order: usize) {
        let block_ptr = addr as *mut BuddyBlock;
        (*block_ptr).next = None;
        
        let block = NonNull::new_unchecked(block_ptr);
        self.free_lists[order].push(block);
        
        let frames = 1 << order;
        self.free_frames += frames;
    }

    /// Calculate the buddy address for a given block
    fn buddy_addr(&self, addr: usize, order: usize) -> usize {
        let block_size = (1 << order) * PAGE_SIZE;
        let relative_addr = addr - self.base_addr.value();
        let buddy_offset = relative_addr ^ block_size;
        self.base_addr.value() + buddy_offset
    }

    /// Allocate a single frame (order 0)
    pub fn alloc_frame(&mut self) -> MemoryResult<PhysicalAddress> {
        self.alloc_order(0)
    }

    /// Allocate contiguous frames (automatic order calculation)
    pub fn alloc_contiguous(&mut self, count: usize) -> MemoryResult<PhysicalAddress> {
        if count == 0 {
            return Err(MemoryError::InvalidSize);
        }
        
        // Calculate required order (round up to power of 2)
        let mut order = 0;
        while (1 << order) < count && order < MAX_ORDER {
            order += 1;
        }
        
        if (1 << order) < count {
            return Err(MemoryError::InvalidSize);
        }
        
        self.alloc_order(order)
    }

    /// Allocate a block of specific order
    fn alloc_order(&mut self, order: usize) -> MemoryResult<PhysicalAddress> {
        if order > MAX_ORDER {
            return Err(MemoryError::InvalidSize);
        }

        // Try to find a block in the requested order
        if let Some(block) = self.free_lists[order].pop() {
            let addr = block.as_ptr() as usize;
            self.mark_used(addr, order);
            return Ok(PhysicalAddress::new(addr));
        }

        // No block available, try to split a larger block
        for split_order in (order + 1)..=MAX_ORDER {
            if let Some(block) = self.free_lists[split_order].pop() {
                // Split this block down to the requested order
                let mut current_addr = block.as_ptr() as usize;
                let mut current_order = split_order;

                while current_order > order {
                    current_order -= 1;
                    let buddy = self.buddy_addr(current_addr, current_order);
                    
                    // Add buddy to free list
                    unsafe {
                        self.add_free_block(buddy, current_order);
                    }
                }

                self.mark_used(current_addr, order);
                return Ok(PhysicalAddress::new(current_addr));
            }
        }

        Err(MemoryError::OutOfMemory)
    }

    /// Mark frames as used in bitmap
    fn mark_used(&mut self, addr: usize, order: usize) {
        let base = self.base_addr.value();
        let start_frame = (addr - base) / PAGE_SIZE;
        let count = 1 << order;
        
        self.bitmap.set_range(start_frame, count);
        self.allocated_frames += count;
        self.free_frames -= count;
    }

    /// Mark frames as free in bitmap
    fn mark_free(&mut self, addr: usize, order: usize) {
        let base = self.base_addr.value();
        let start_frame = (addr - base) / PAGE_SIZE;
        let count = 1 << order;
        
        for i in 0..count {
            self.bitmap.clear(start_frame + i);
        }
        
        self.allocated_frames -= count;
    }

    /// Check if a block is free
    fn is_free(&self, addr: usize, order: usize) -> bool {
        let base = self.base_addr.value();
        let start_frame = (addr - base) / PAGE_SIZE;
        let count = 1 << order;
        
        // All frames in the block must be free
        for i in 0..count {
            if self.bitmap.test(start_frame + i) {
                return false;
            }
        }
        true
    }

    /// Free a block and coalesce with buddy if possible
    pub fn free(&mut self, addr: PhysicalAddress, order: usize) -> MemoryResult<()> {
        if order > MAX_ORDER {
            return Err(MemoryError::InvalidSize);
        }

        let mut current_addr = addr.value();
        let mut current_order = order;

        // Mark frames as free
        self.mark_free(current_addr, order);

        // Try to coalesce with buddy
        while current_order < MAX_ORDER {
            let buddy = self.buddy_addr(current_addr, current_order);
            
            // Check if buddy is free and in the free list
            if !self.is_free(buddy, current_order) {
                break;
            }

            // Try to remove buddy from free list
            unsafe {
                let buddy_ptr = buddy as *mut BuddyBlock;
                if let Some(buddy_block) = NonNull::new(buddy_ptr) {
                    if self.free_lists[current_order].remove(buddy_block) {
                        // Coalesce: use lower address as new block
                        current_addr = current_addr.min(buddy);
                        current_order += 1;
                        self.free_frames -= 1 << (current_order - 1);
                        continue;
                    }
                }
            }
            
            break;
        }

        // Add final block to free list
        unsafe {
            self.add_free_block(current_addr, current_order);
        }

        Ok(())
    }

    /// Free a single frame
    pub fn free_frame(&mut self, addr: PhysicalAddress) -> MemoryResult<()> {
        self.free(addr, 0)
    }

    /// Free contiguous frames
    pub fn free_contiguous(&mut self, addr: PhysicalAddress, count: usize) -> MemoryResult<()> {
        if count == 0 {
            return Ok(());
        }
        
        // Calculate order
        let mut order = 0;
        while (1 << order) < count && order < MAX_ORDER {
            order += 1;
        }
        
        self.free(addr, order)
    }

    /// Get statistics
    pub fn stats(&self) -> BuddyStats {
        let mut free_by_order = [0usize; NUM_ORDERS];
        for order in 0..NUM_ORDERS {
            free_by_order[order] = self.free_lists[order].count;
        }

        BuddyStats {
            total_frames: self.total_frames,
            allocated_frames: self.allocated_frames,
            free_frames: self.free_frames,
            free_by_order,
        }
    }
}

/// Statistics for buddy allocator
#[derive(Debug, Clone)]
pub struct BuddyStats {
    pub total_frames: usize,
    pub allocated_frames: usize,
    pub free_frames: usize,
    pub free_by_order: [usize; NUM_ORDERS],
}

impl BuddyStats {
    pub fn usage_percent(&self) -> f32 {
        if self.total_frames == 0 {
            0.0
        } else {
            (self.allocated_frames as f32 / self.total_frames as f32) * 100.0
        }
    }
}

/// Global buddy allocator instance
static BUDDY: Mutex<Option<BuddyAllocator>> = Mutex::new(None);

/// Initialize the global buddy allocator
pub unsafe fn init(
    bitmap_addr: usize,
    bitmap_size: usize,
    memory_start: PhysicalAddress,
    total_size: usize,
) {
    let allocator = BuddyAllocator::new(bitmap_addr, bitmap_size, memory_start, total_size);
    *BUDDY.lock() = Some(allocator);
}

/// Add a free region to the buddy allocator
pub fn add_free_region(start: PhysicalAddress, size: usize) {
    if let Some(ref mut buddy) = *BUDDY.lock() {
        buddy.add_region(start, size);
    }
}

/// Allocate a physical frame
pub fn alloc_frame() -> MemoryResult<PhysicalAddress> {
    BUDDY.lock()
        .as_mut()
        .ok_or(MemoryError::InternalError("Buddy allocator not initialized"))?
        .alloc_frame()
}

/// Free a physical frame
pub fn free_frame(addr: PhysicalAddress) -> MemoryResult<()> {
    BUDDY.lock()
        .as_mut()
        .ok_or(MemoryError::InternalError("Buddy allocator not initialized"))?
        .free_frame(addr)
}

/// Allocate contiguous frames
pub fn alloc_contiguous(count: usize) -> MemoryResult<PhysicalAddress> {
    BUDDY.lock()
        .as_mut()
        .ok_or(MemoryError::InternalError("Buddy allocator not initialized"))?
        .alloc_contiguous(count)
}

/// Free contiguous frames
pub fn free_contiguous(addr: PhysicalAddress, count: usize) -> MemoryResult<()> {
    BUDDY.lock()
        .as_mut()
        .ok_or(MemoryError::InternalError("Buddy allocator not initialized"))?
        .free_contiguous(addr, count)
}

/// Get buddy allocator statistics
pub fn get_stats() -> Option<BuddyStats> {
    BUDDY.lock().as_ref().map(|b| b.stats())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buddy_alloc_free() {
        // This would require a test harness with real memory
        // For now, document the expected behavior
    }
}
