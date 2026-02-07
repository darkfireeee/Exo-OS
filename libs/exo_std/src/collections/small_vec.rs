// libs/exo_std/src/collections/small_vec.rs
//! SmallVec: Vector optimized with inline storage for small sizes
//!
//! SmallVec avoids heap allocations for small vectors by storing them
//! directly in the structure. Only if the size exceeds N elements does
//! it transition to heap allocation.

use core::ops::{Deref, DerefMut, Index, IndexMut};
use core::ptr;
use core::mem::{MaybeUninit, ManuallyDrop};
use core::slice;
use core::fmt;
use super::bounded_vec::CapacityError;

extern crate alloc;
use alloc::vec::Vec;

/// SmallVec with inline storage up to N elements
///
/// # Example
/// ```no_run
/// use exo_std::collections::SmallVec;
///
/// // Can store up to 8 elements inline without allocation
/// let mut vec: SmallVec<u32, 8> = SmallVec::new();
///
/// vec.push(1);
/// vec.push(2);
/// // No allocation as long as <= 8 elements
/// ```
pub struct SmallVec<T, const N: usize> {
    /// Current length
    len: usize,
    /// Inline or heap storage
    data: SmallVecData<T, N>,
}

union SmallVecData<T, const N: usize> {
    /// Inline storage for <= N elements
    inline: ManuallyDrop<[MaybeUninit<T>; N]>,
    /// Heap storage for > N elements
    heap: ManuallyDrop<Vec<T>>,
}

impl<T, const N: usize> SmallVec<T, N> {
    /// Create new empty SmallVec
    #[inline]
    pub const fn new() -> Self {
        Self {
            len: 0,
            data: SmallVecData {
                inline: ManuallyDrop::new(unsafe { MaybeUninit::uninit().assume_init() }),
            },
        }
    }

    /// Create with capacity
    pub fn with_capacity(capacity: usize) -> Self {
        if capacity <= N {
            Self::new()
        } else {
            Self {
                len: 0,
                data: SmallVecData {
                    heap: ManuallyDrop::new(Vec::with_capacity(capacity)),
                },
            }
        }
    }

    /// Check if using inline storage
    #[inline]
    pub const fn is_inline(&self) -> bool {
        self.len <= N
    }

    /// Add an element
    #[inline]
    pub fn push(&mut self, value: T) {
        if self.len < N {
            // Inline push
            unsafe {
                (*self.data.inline)[self.len] = MaybeUninit::new(value);
            }
            self.len += 1;
        } else if self.len == N {
            // Transition to heap
            let mut vec = Vec::with_capacity(N * 2);
            unsafe {
                for i in 0..N {
                    let val = (*self.data.inline)[i].assume_init_read();
                    vec.push(val);
                }
            }
            vec.push(value);
            self.data = SmallVecData {
                heap: ManuallyDrop::new(vec),
            };
            self.len = N + 1;
        } else {
            // Heap push
            unsafe {
                (*self.data.heap).push(value);
            }
            self.len += 1;
        }
    }

    /// Try to push with capacity check (when transitioning would fail)
    #[inline]
    pub fn try_push(&mut self, value: T) -> Result<(), CapacityError> {
        if self.len < N {
            unsafe {
                (*self.data.inline)[self.len] = MaybeUninit::new(value);
            }
            self.len += 1;
            Ok(())
        } else {
            Err(CapacityError)
        }
    }

    /// Remove and return the last element
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }

        self.len -= 1;

        if self.len < N {
            // Inline
            unsafe {
                let value = (*self.data.inline)[self.len].assume_init_read();
                Some(value)
            }
        } else {
            // Heap
            unsafe { (*self.data.heap).pop() }
        }
    }

    /// Insert at given index
    #[inline]
    pub fn insert(&mut self, index: usize, value: T) {
        assert!(index <= self.len, "index out of bounds");

        if self.len < N {
            // Inline insert
            unsafe {
                let ptr = (*self.data.inline).as_mut_ptr();
                let insert_ptr = ptr.add(index);
                ptr::copy(insert_ptr, insert_ptr.add(1), self.len - index);
                (*insert_ptr) = MaybeUninit::new(value);
            }
            self.len += 1;
        } else {
            // Heap or transition needed
            if self.len == N {
                // Need to transition to heap first
                let mut vec = Vec::with_capacity(N * 2);
                unsafe {
                    for i in 0..N {
                        let val = (*self.data.inline)[i].assume_init_read();
                        vec.push(val);
                    }
                }
                vec.insert(index, value);
                self.data = SmallVecData {
                    heap: ManuallyDrop::new(vec),
                };
                self.len = N + 1;
            } else {
                unsafe {
                    (*self.data.heap).insert(index, value);
                }
                self.len += 1;
            }
        }
    }

    /// Remove element at given index
    #[inline]
    pub fn remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "index out of bounds");

        self.len -= 1;

        if self.len < N {
            unsafe {
                let ptr = (*self.data.inline).as_mut_ptr() as *mut T;
                let remove_ptr = ptr.add(index);
                let value = ptr::read(remove_ptr);
                ptr::copy(remove_ptr.add(1), remove_ptr, self.len - index);
                value
            }
        } else {
            unsafe { (*self.data.heap).remove(index) }
        }
    }

    /// Swap remove (faster, doesn't preserve order)
    #[inline]
    pub fn swap_remove(&mut self, index: usize) -> T {
        assert!(index < self.len, "index out of bounds");

        self.len -= 1;

        if self.len < N {
            unsafe {
                let ptr = (*self.data.inline).as_mut_ptr() as *mut T;
                let remove_ptr = ptr.add(index);
                let value = ptr::read(remove_ptr);

                if index != self.len {
                    ptr::copy(ptr.add(self.len), remove_ptr, 1);
                }

                value
            }
        } else {
            unsafe { (*self.data.heap).swap_remove(index) }
        }
    }

    /// Clear all elements
    #[inline]
    pub fn clear(&mut self) {
        if self.len <= N {
            // Inline
            unsafe {
                for i in 0..self.len {
                    (*self.data.inline)[i].assume_init_drop();
                }
            }
        } else {
            // Heap
            unsafe {
                (*self.data.heap).clear();
            }
        }
        self.len = 0;
    }

    /// Truncate to len elements
    #[inline]
    pub fn truncate(&mut self, len: usize) {
        if len < self.len {
            if self.len <= N {
                unsafe {
                    for i in len..self.len {
                        (*self.data.inline)[i].assume_init_drop();
                    }
                }
            } else {
                unsafe {
                    (*self.data.heap).truncate(len);
                }
            }
            self.len = len;
        }
    }

    /// Extend from slice
    pub fn extend_from_slice(&mut self, other: &[T])
    where
        T: Clone,
    {
        for item in other {
            self.push(item.clone());
        }
    }

    /// Keep only elements satisfying predicate
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&T) -> bool,
    {
        let mut i = 0;
        while i < self.len {
            if !f(&self[i]) {
                self.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Deduplicate consecutive equal elements
    pub fn dedup(&mut self)
    where
        T: PartialEq,
    {
        let mut i = 1;
        while i < self.len {
            if self[i] == self[i - 1] {
                self.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Access element
    #[inline]
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }

        if self.len <= N {
            // Inline
            unsafe {
                Some((*self.data.inline)[index].assume_init_ref())
            }
        } else {
            // Heap
            unsafe { (*self.data.heap).get(index) }
        }
    }

    /// Mutable access
    #[inline]
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.len {
            return None;
        }

        if self.len <= N {
            // Inline
            unsafe {
                Some((*self.data.inline)[index].assume_init_mut())
            }
        } else {
            // Heap
            unsafe { (*self.data.heap).get_mut(index) }
        }
    }

    /// First element
    #[inline]
    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }

    /// Last element
    #[inline]
    pub fn last(&self) -> Option<&T> {
        if self.len > 0 {
            self.get(self.len - 1)
        } else {
            None
        }
    }

    /// Length
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Check if empty
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Capacity
    #[inline]
    pub fn capacity(&self) -> usize {
        if self.len <= N {
            N
        } else {
            unsafe { (*self.data.heap).capacity() }
        }
    }

    /// Remaining capacity
    #[inline]
    pub fn remaining(&self) -> usize {
        self.capacity().saturating_sub(self.len)
    }

    /// Check if full (only for inline)
    #[inline]
    pub const fn is_full(&self) -> bool {
        self.len >= N
    }

    /// Convert to slice
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        if self.len <= N {
            unsafe {
                slice::from_raw_parts(
                    (*self.data.inline).as_ptr() as *const T,
                    self.len,
                )
            }
        } else {
            unsafe { (*self.data.heap).as_slice() }
        }
    }

    /// Convert to mutable slice
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.len <= N {
            unsafe {
                slice::from_raw_parts_mut(
                    (*self.data.inline).as_mut_ptr() as *mut T,
                    self.len,
                )
            }
        } else {
            unsafe { (*self.data.heap).as_mut_slice() }
        }
    }
}

impl<T, const N: usize> Deref for SmallVec<T, N> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, const N: usize> DerefMut for SmallVec<T, N> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, I: slice::SliceIndex<[T]>, const N: usize> Index<I> for SmallVec<T, N> {
    type Output = I::Output;

    #[inline]
    fn index(&self, index: I) -> &Self::Output {
        &self.as_slice()[index]
    }
}

impl<T, I: slice::SliceIndex<[T]>, const N: usize> IndexMut<I> for SmallVec<T, N> {
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        &mut self.as_mut_slice()[index]
    }
}

impl<T, const N: usize> Drop for SmallVec<T, N> {
    fn drop(&mut self) {
        self.clear();
        if self.capacity() > N {
            unsafe {
                ManuallyDrop::drop(&mut self.data.heap);
            }
        }
    }
}

impl<T: fmt::Debug, const N: usize> fmt::Debug for SmallVec<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: Clone, const N: usize> Clone for SmallVec<T, N> {
    fn clone(&self) -> Self {
        let mut new = Self::new();
        for item in self.as_slice() {
            new.push(item.clone());
        }
        new
    }
}

impl<T, const N: usize> Default for SmallVec<T, N> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<T: Send, const N: usize> Send for SmallVec<T, N> {}
unsafe impl<T: Sync, const N: usize> Sync for SmallVec<T, N> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_vec_inline() {
        let mut vec: SmallVec<u32, 8> = SmallVec::new();

        assert!(vec.is_inline());
        assert!(vec.is_empty());

        vec.push(1);
        vec.push(2);
        vec.push(3);

        assert_eq!(vec.len(), 3);
        assert_eq!(vec[0], 1);
        assert_eq!(vec[1], 2);
        assert_eq!(vec[2], 3);

        assert_eq!(vec.pop(), Some(3));
        assert_eq!(vec.len(), 2);
    }

    #[test]
    fn test_small_vec_transition() {
        let mut vec: SmallVec<i32, 2> = SmallVec::new();

        vec.push(1);
        vec.push(2);
        assert!(vec.is_inline());

        vec.push(3); // Transition to heap
        assert!(!vec.is_inline());
        vec.push(4);

        assert_eq!(vec.len(), 4);
        assert!(vec.capacity() > 2);
        assert_eq!(vec.get(0), Some(&1));
        assert_eq!(vec.get(1), Some(&2));
        assert_eq!(vec.get(2), Some(&3));
        assert_eq!(vec.pop(), Some(4));
        assert_eq!(vec.len(), 3);
    }

    #[test]
    fn test_small_vec_swap_remove() {
        let mut vec: SmallVec<u32, 8> = SmallVec::new();
        vec.push(1);
        vec.push(2);
        vec.push(3);
        vec.push(4);

        let removed = vec.swap_remove(1);
        assert_eq!(removed, 2);
        assert_eq!(vec.as_slice(), &[1, 4, 3]);
    }

    #[test]
    fn test_small_vec_insert_remove() {
        let mut vec: SmallVec<u32, 8> = SmallVec::new();
        vec.push(1);
        vec.push(3);
        vec.insert(1, 2);

        assert_eq!(vec.as_slice(), &[1, 2, 3]);

        let removed = vec.remove(1);
        assert_eq!(removed, 2);
        assert_eq!(vec.as_slice(), &[1, 3]);
    }

    #[test]
    fn test_small_vec_retain() {
        let mut vec: SmallVec<u32, 8> = SmallVec::new();
        vec.push(1);
        vec.push(2);
        vec.push(3);
        vec.push(4);

        vec.retain(|&x| x % 2 == 0);
        assert_eq!(vec.as_slice(), &[2, 4]);
    }

    #[test]
    fn test_small_vec_dedup() {
        let mut vec: SmallVec<u32, 8> = SmallVec::new();
        vec.push(1);
        vec.push(1);
        vec.push(2);
        vec.push(2);
        vec.push(3);

        vec.dedup();
        assert_eq!(vec.as_slice(), &[1, 2, 3]);
    }
}
