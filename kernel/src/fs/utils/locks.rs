//! Lock-free primitives and synchronization utilities

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Simple spinlock based on atomic bool
pub struct SpinLock {
    locked: AtomicBool,
}

impl SpinLock {
    /// Create new unlocked spinlock
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    /// Try to acquire lock (non-blocking)
    #[inline]
    pub fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Acquire lock (blocking with spin)
    pub fn lock(&self) {
        while !self.try_lock() {
            core::hint::spin_loop();
        }
    }

    /// Release lock
    #[inline]
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }

    /// Check if locked
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }
}

/// Sequence lock for lock-free reads
pub struct SeqLock {
    sequence: AtomicU32,
}

impl SeqLock {
    /// Create new sequence lock
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
        }
    }

    /// Begin write (returns sequence number)
    #[inline]
    pub fn write_begin(&self) -> u32 {
        let seq = self.sequence.fetch_add(1, Ordering::Acquire);
        core::sync::atomic::fence(Ordering::Release);
        seq
    }

    /// End write
    #[inline]
    pub fn write_end(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
    }

    /// Begin read (returns sequence number)
    #[inline]
    pub fn read_begin(&self) -> u32 {
        loop {
            let seq = self.sequence.load(Ordering::Acquire);
            if seq & 1 == 0 {
                return seq;
            }
            core::hint::spin_loop();
        }
    }

    /// Check if read is still valid
    #[inline]
    pub fn read_retry(&self, seq: u32) -> bool {
        core::sync::atomic::fence(Ordering::Acquire);
        self.sequence.load(Ordering::Relaxed) != seq
    }
}

/// Simple atomic counter
pub struct AtomicCounter {
    count: AtomicU64,
}

impl AtomicCounter {
    /// Create new counter
    pub const fn new(initial: u64) -> Self {
        Self {
            count: AtomicU64::new(initial),
        }
    }

    /// Increment and return old value
    #[inline]
    pub fn fetch_inc(&self) -> u64 {
        self.count.fetch_add(1, Ordering::Relaxed)
    }

    /// Increment and return new value
    #[inline]
    pub fn inc(&self) -> u64 {
        self.count.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Decrement and return old value
    #[inline]
    pub fn fetch_dec(&self) -> u64 {
        self.count.fetch_sub(1, Ordering::Relaxed)
    }

    /// Get current value
    #[inline]
    pub fn get(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Set value
    #[inline]
    pub fn set(&self, val: u64) {
        self.count.store(val, Ordering::Relaxed);
    }

    /// Add to counter
    #[inline]
    pub fn add(&self, val: u64) -> u64 {
        self.count.fetch_add(val, Ordering::Relaxed) + val
    }

    /// Subtract from counter
    #[inline]
    pub fn sub(&self, val: u64) -> u64 {
        self.count.fetch_sub(val, Ordering::Relaxed) - val
    }
}

/// Reference counting using atomics
pub struct AtomicRefCount {
    count: AtomicU32,
}

impl AtomicRefCount {
    /// Create with count of 1
    pub const fn new() -> Self {
        Self {
            count: AtomicU32::new(1),
        }
    }

    /// Increment reference count
    #[inline]
    pub fn inc(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement reference count, returns true if dropped to zero
    #[inline]
    pub fn dec(&self) -> bool {
        self.count.fetch_sub(1, Ordering::Release) == 1
    }

    /// Get current count
    #[inline]
    pub fn get(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }
}
