//! Frame Reference Counting
//!
//! This module provides a mechanism to track the number of references to physical frames.
//! This is essential for Copy-On-Write (COW) implementation, where multiple virtual pages
//! may map to the same physical frame.

use crate::memory::PhysicalAddress;
use alloc::collections::BTreeMap;
use spin::Mutex;

/// Global frame tracker
pub static FRAME_TRACKER: Mutex<FrameTracker> = Mutex::new(FrameTracker::new());

/// Tracks reference counts for physical frames
pub struct FrameTracker {
    counts: BTreeMap<usize, usize>,
}

impl FrameTracker {
    /// Create a new empty tracker
    pub const fn new() -> Self {
        Self {
            counts: BTreeMap::new(),
        }
    }

    /// Increment reference count for a frame
    pub fn ref_frame(&mut self, addr: PhysicalAddress) {
        let val = addr.value();
        // If not in map, it implies 1 reference exists (the allocator/owner).
        // We are adding another one, so we start at 1 and increment to 2.
        let count = self.counts.entry(val).or_insert(1);
        *count += 1;
    }

    /// Decrement reference count for a frame
    /// Returns true if the count reaches 0 (meaning the frame should be freed)
    pub fn unref_frame(&mut self, addr: PhysicalAddress) -> bool {
        let val = addr.value();
        if let Some(count) = self.counts.get_mut(&val) {
            *count -= 1;
            if *count == 0 {
                self.counts.remove(&val);
                return true;
            }
            return false;
        }
        // If not tracked, assume it was 1 and now 0 (should be freed)
        // This handles frames allocated before tracking started or not explicitly tracked
        true
    }

    /// Get current reference count
    pub fn get_ref_count(&self, addr: PhysicalAddress) -> usize {
        *self.counts.get(&addr.value()).unwrap_or(&1)
    }
}

/// Helper to increment reference count
pub fn ref_frame(addr: PhysicalAddress) {
    FRAME_TRACKER.lock().ref_frame(addr);
}

/// Helper to decrement reference count
/// Returns true if the frame should be freed
pub fn unref_frame(addr: PhysicalAddress) -> bool {
    FRAME_TRACKER.lock().unref_frame(addr)
}
