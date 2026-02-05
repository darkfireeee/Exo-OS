//! Slab allocator for fixed-size objects
//!
//! Optimized for allocating/freeing objects of the same size (descriptors, tasks, buffers).

use crate::{AllocError, Result};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Free list node (stored in freed slots)
#[repr(C)]
struct FreeNode {
    next: Option<NonNull<FreeNode>>,
}

/// Slab allocator for fixed-size objects
pub struct SlabAllocator {
    /// Base pointer to memory region
    base: Option<NonNull<u8>>,
    /// Size of each object
    object_size: usize,
    /// Total capacity
    capacity: usize,
    /// Free list head
    free_list: Option<NonNull<FreeNode>>,
    /// Number of allocated objects
    allocated: AtomicUsize,
}

impl SlabAllocator {
    /// Create new slab allocator (uninitialized)
    ///
    /// Must call `init()` before use
    pub const fn new(object_size: usize, capacity: usize) -> Self {
        assert!(object_size >= core::mem::size_of::<FreeNode>());
        assert!(object_size % core::mem::align_of::<FreeNode>() == 0);
        
        Self {
            base: None,
            object_size,
            capacity,
            free_list: None,
            allocated: AtomicUsize::new(0),
        }
    }
    
    /// Initialize with memory region
    ///
    /// # Safety
    /// - `base` must point to valid memory of at least `object_size * capacity` bytes
    /// - Memory must be properly aligned for `object_size`
    pub unsafe fn init(&mut self, base: NonNull<u8>) {
        self.base = Some(base);
        
        // Build free list
        let mut current = base.as_ptr();
        for i in 0..self.capacity {
            let node = current as *mut FreeNode;
            
            let next_offset = (i + 1) * self.object_size;
            if next_offset < self.capacity * self.object_size {
                unsafe {
                    let next_ptr = base.as_ptr().add(next_offset) as *mut FreeNode;
                    (*node).next = NonNull::new(next_ptr);
                }
            } else {
                unsafe {
                    (*node).next = None;
                }
            }
            
            current = unsafe { current.add(self.object_size) };
        }
        
        self.free_list = NonNull::new(base.as_ptr() as *mut FreeNode);
    }

    /// Allocate one object
    ///
    /// Returns pointer to uninitialized memory of `object_size` bytes
    pub fn alloc(&mut self) -> Result<NonNull<u8>> {
        if let Some(node_ptr) = self.free_list {
            let node = unsafe { node_ptr.as_ref() };
            self.free_list = node.next;
            self.allocated.fetch_add(1, Ordering::Relaxed);
            Ok(node_ptr.cast())
        } else {
            Err(AllocError::OutOfMemory)
        }
    }

    /// Free previously allocated object
    ///
    /// # Safety
    /// - Pointer must have been returned by `alloc()` from this allocator
    /// - Pointer must not already be freed (double-free)
    pub unsafe fn free(&mut self, ptr: NonNull<u8>) {
        let node = ptr.cast::<FreeNode>().as_ptr();
        unsafe {
            (*node).next = self.free_list;
            self.free_list = Some(NonNull::new_unchecked(node));
        }
        self.allocated.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get object size
    pub const fn object_size(&self) -> usize {
        self.object_size
    }

    /// Get capacity
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
    
    /// Get number of allocated objects
    pub fn allocated(&self) -> usize {
        self.allocated.load(Ordering::Relaxed)
    }
    
    /// Get number of free objects
    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.allocated())
    }
    
    /// Check if full
    pub fn is_full(&self) -> bool {
        self.allocated() >= self.capacity
    }
    
    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.allocated() == 0
    }
}

unsafe impl Send for SlabAllocator {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slab_basic() {
        const SIZE: usize = 64;
        const COUNT: usize = 10;
        let mut backing = vec![0u8; SIZE * COUNT];
        let base = NonNull::new(backing.as_mut_ptr()).unwrap();
        
        let mut slab = SlabAllocator::new(SIZE, COUNT);
        unsafe { slab.init(base); }
        
        assert_eq!(slab.available(), COUNT);
        assert!(slab.is_empty());
        
        let ptr1 = slab.alloc().unwrap();
        assert_eq!(slab.allocated(), 1);
        
        let ptr2 = slab.alloc().unwrap();
        assert_eq!(slab.allocated(), 2);
        
        unsafe { slab.free(ptr1); }
        assert_eq!(slab.allocated(), 1);
        
        unsafe { slab.free(ptr2); }
        assert!(slab.is_empty());
    }
    
    #[test]
    fn test_slab_creation() {
        let slab = SlabAllocator::new(64, 1024);
        assert_eq!(slab.object_size(), 64);
        assert_eq!(slab.capacity(), 1024);
    }
}
