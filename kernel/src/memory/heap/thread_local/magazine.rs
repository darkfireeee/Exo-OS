// kernel/src/memory/heap/thread_local/magazine.rs
//
// Per-CPU magazine cache — pre-allocated frames buffer to avoid locks.
// Inspired by the Solaris/Bonwick magazine protocol.
// Layer 0 — no external dependencies except `spin`.

use core::ptr::NonNull;

/// Size of a magazine (objects cached per CPU).
pub const MAGAZINE_SIZE: usize = 64;

/// A magazine = array of pointers to pre-allocated free objects.
///
/// Two magazines per CPU: one "loaded" (ready to allocate) and one "prev"
/// (ready to be filled). The swap between them is atomic O(1).
#[repr(C, align(64))]
pub struct Magazine {
    /// Array of free objects (indices 0..count-1 are valid).
    objects: [*mut u8; MAGAZINE_SIZE],
    /// Number of objects currently in the magazine.
    count: usize,
    /// Size class of this magazine.
    pub size_class: usize,
}

// SAFETY: Magazine is used by a single CPU at a time (per-CPU).
unsafe impl Send for Magazine {}
unsafe impl Sync for Magazine {}

impl Magazine {
    pub const fn new(size_class: usize) -> Self {
        Magazine {
            objects: [core::ptr::null_mut(); MAGAZINE_SIZE],
            count: 0,
            size_class,
        }
    }

    /// Pops an object from the magazine (O(1)).
    /// Returns `None` if the magazine is empty.
    #[inline]
    pub fn pop(&mut self) -> Option<NonNull<u8>> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        NonNull::new(self.objects[self.count])
    }

    /// Pushes an object into the magazine (O(1)).
    /// Returns `false` if the magazine is full.
    #[inline]
    pub fn push(&mut self, ptr: NonNull<u8>) -> bool {
        if self.count >= MAGAZINE_SIZE {
            return false;
        }
        self.objects[self.count] = ptr.as_ptr();
        self.count += 1;
        true
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    #[inline]
    pub fn is_full(&self) -> bool {
        self.count >= MAGAZINE_SIZE
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.count
    }
    #[inline]
    pub fn capacity(&self) -> usize {
        MAGAZINE_SIZE
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PER-CPU MAGAZINE PAIR
// ─────────────────────────────────────────────────────────────────────────────

/// Per-CPU magazine pair for a size class.
///
/// Protocol:
/// 1. Alloc: pop() on `loaded`. If empty, swap loaded<->prev and pop on new loaded.
///    If still empty, fall back to central depot.
/// 2. Free: push() on `loaded`. If full, swap loaded<->prev and push on new loaded.
///    If still full, return prev to depot and allocate a new one.
pub struct CpuMagazinePair {
    pub loaded: Magazine,
    pub prev: Magazine,
}

impl CpuMagazinePair {
    pub const fn new(size_class: usize) -> Self {
        CpuMagazinePair {
            loaded: Magazine::new(size_class),
            prev: Magazine::new(size_class),
        }
    }

    /// Allocates an object via the magazine.
    /// Returns `None` if both magazines are empty (refill required).
    #[inline]
    pub fn alloc(&mut self) -> Option<NonNull<u8>> {
        if let Some(p) = self.loaded.pop() {
            return Some(p);
        }
        // Swap and retry
        core::mem::swap(&mut self.loaded, &mut self.prev);
        self.loaded.pop()
    }

    /// Frees an object via the magazine.
    /// Returns `false` if both magazines are full (drain required).
    #[inline]
    pub fn free(&mut self, ptr: NonNull<u8>) -> bool {
        if self.loaded.push(ptr) {
            return true;
        }
        // Swap and retry
        core::mem::swap(&mut self.loaded, &mut self.prev);
        self.loaded.push(ptr)
    }

    /// Returns the total number of cached objects on this CPU.
    #[inline]
    pub fn cached_count(&self) -> usize {
        self.loaded.len() + self.prev.len()
    }
}
