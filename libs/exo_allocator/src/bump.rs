//! Bump allocator (arena allocator) for temporary allocations
//!
//! Extremely fast allocator that bumps a pointer. All allocations freed together.

use crate::{AllocError, Result};
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Bump/arena allocator
pub struct BumpAllocator {
    /// Base pointer
    base: Option<NonNull<u8>>,
    /// Capacity in bytes
    capacity: usize,
    /// Current offset (atomically updated)
    offset: AtomicUsize,
}

impl BumpAllocator {
    /// Create bump allocator (uninitialized)
    pub const fn new() -> Self {
        Self {
            base: None,
            capacity: 0,
            offset: AtomicUsize::new(0),
        }
    }

    /// Create bump allocator with capacity
    pub const fn with_capacity(capacity: usize) -> Self {
        Self {
            base: None,
            capacity,
            offset: AtomicUsize::new(0),
        }
    }

    /// Initialize with memory region
    ///
    /// # Safety
    /// `base` must point to valid memory of at least `capacity` bytes
    pub unsafe fn init(&mut self, base: NonNull<u8>, capacity: usize) {
        self.base = Some(base);
        self.capacity = capacity;
        self.offset.store(0, Ordering::Release);
    }

    /// Allocate byte slice
    ///
    /// # Safety
    /// All allocations must be dropped before allocator reset
    pub unsafe fn alloc(&self, size: usize, align: usize) -> Result<NonNull<u8>> {
        let base = self.base.ok_or(AllocError::InvalidState)?;

        // Align current offset
        let current = self.offset.load(Ordering::Acquire);
        let aligned = align_up(current, align);
        let new_offset = aligned.checked_add(size).ok_or(AllocError::Overflow)?;

        if new_offset > self.capacity {
            return Err(AllocError::OutOfMemory);
        }

        // Try to claim this range (CAS loop for thread safety)
        match self.offset.compare_exchange_weak(
            current,
            new_offset,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                let ptr = unsafe { base.as_ptr().add(aligned) };
                Ok(unsafe { NonNull::new_unchecked(ptr) })
            }
            Err(_) => {
                // Retry on contention
                unsafe { self.alloc(size, align) }
            }
        }
    }

    /// Allocate typed value
    pub fn alloc_value<T>(&self, value: T) -> Result<&mut T> {
        let ptr = unsafe { self.alloc(core::mem::size_of::<T>(), core::mem::align_of::<T>())? };
        let typed = ptr.cast::<T>().as_ptr();
        unsafe {
            ptr::write(typed, value);
            Ok(&mut *typed)
        }
    }

    /// Allocate and copy slice
    pub fn alloc_slice<T: Copy>(&self, data: &[T]) -> Result<&mut [T]> {
        let size = core::mem::size_of::<T>() * data.len();
        let align = core::mem::align_of::<T>();
        let ptr = unsafe { self.alloc(size, align)? };
        let typed = ptr.cast::<T>().as_ptr();
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), typed, data.len());
            Ok(core::slice::from_raw_parts_mut(typed, data.len()))
        }
    }

    /// Allocate string slice
    pub fn alloc_str(&self, s: &str) -> Result<&mut str> {
        let bytes = self.alloc_slice(s.as_bytes())?;
        Ok(unsafe { core::str::from_utf8_unchecked_mut(bytes) })
    }

    /// Reset allocator (free all allocations)
    ///
    /// # Safety
    /// All previously allocated pointers become invalid
    pub unsafe fn reset(&mut self) {
        self.offset.store(0, Ordering::Release);
    }

    /// Get current usage bytes
    pub fn used(&self) -> usize {
        self.offset.load(Ordering::Acquire)
    }

    /// Get capacity
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get available bytes
    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.used())
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.used() == 0
    }
}

/// Align value up to alignment
#[inline]
const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

unsafe impl Send for BumpAllocator {}
unsafe impl Sync for BumpAllocator {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bump_basic() {
        let mut backing = vec![0u8; 1024];
        let base = NonNull::new(backing.as_mut_ptr()).unwrap();

        let mut bump = BumpAllocator::new();
        unsafe {
            bump.init(base, 1024);
        }

        assert_eq!(bump.used(), 0);
        assert_eq!(bump.available(), 1024);

        unsafe {
            let ptr1 = bump.alloc(64, 8).unwrap();
            assert_eq!(bump.used(), 64);

            let ptr2 = bump.alloc(32, 8).unwrap();
            assert_eq!(bump.used(), 96);

            bump.reset();
            assert_eq!(bump.used(), 0);
        }
    }

    #[test]
    fn test_bump_typed() {
        let mut backing = vec![0u8; 1024];
        let base = NonNull::new(backing.as_mut_ptr()).unwrap();

        let mut bump = BumpAllocator::new();
        unsafe {
            bump.init(base, 1024);
        }

        let val = bump.alloc_value(42u64).unwrap();
        assert_eq!(*val, 42);

        let slice = bump.alloc_slice(&[1, 2, 3, 4, 5]).unwrap();
        assert_eq!(slice, &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_bump_creation() {
        let bump = BumpAllocator::with_capacity(4096);
        assert_eq!(bump.capacity(), 4096);
        assert_eq!(bump.used(), 0);
    }
}
