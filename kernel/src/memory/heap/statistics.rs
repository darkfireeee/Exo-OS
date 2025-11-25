//! Heap Allocator Statistics

use core::sync::atomic::{AtomicUsize, Ordering};

/// Global allocation statistics
pub struct AllocatorStatistics {
    pub thread_local_allocs: AtomicUsize,
    pub thread_local_deallocs: AtomicUsize,
    pub cpu_slab_allocs: AtomicUsize,
    pub cpu_slab_deallocs: AtomicUsize,
    pub buddy_allocs: AtomicUsize,
    pub buddy_deallocs: AtomicUsize,
    pub total_allocated_bytes: AtomicUsize,
    pub total_freed_bytes: AtomicUsize,
}

impl AllocatorStatistics {
    pub const fn new() -> Self {
        Self {
            thread_local_allocs: AtomicUsize::new(0),
            thread_local_deallocs: AtomicUsize::new(0),
            cpu_slab_allocs: AtomicUsize::new(0),
            cpu_slab_deallocs: AtomicUsize::new(0),
            buddy_allocs: AtomicUsize::new(0),
            buddy_deallocs: AtomicUsize::new(0),
            total_allocated_bytes: AtomicUsize::new(0),
            total_freed_bytes: AtomicUsize::new(0),
        }
    }

    pub fn record_thread_alloc(&self, size: usize) {
        self.thread_local_allocs.fetch_add(1, Ordering::Relaxed);
        self.total_allocated_bytes.fetch_add(size, Ordering::Relaxed);
    }

    pub fn record_thread_dealloc(&self, size: usize) {
        self.thread_local_deallocs.fetch_add(1, Ordering::Relaxed);
        self.total_freed_bytes.fetch_add(size, Ordering::Relaxed);
    }

    pub fn record_cpu_alloc(&self, size: usize) {
        self.cpu_slab_allocs.fetch_add(1, Ordering::Relaxed);
        self.total_allocated_bytes.fetch_add(size, Ordering::Relaxed);
    }

    pub fn record_cpu_dealloc(&self, size: usize) {
        self.cpu_slab_deallocs.fetch_add(1, Ordering::Relaxed);
        self.total_freed_bytes.fetch_add(size, Ordering::Relaxed);
    }

    pub fn record_buddy_alloc(&self, size: usize) {
        self.buddy_allocs.fetch_add(1, Ordering::Relaxed);
        self.total_allocated_bytes.fetch_add(size, Ordering::Relaxed);
    }

    pub fn record_buddy_dealloc(&self, size: usize) {
        self.buddy_deallocs.fetch_add(1, Ordering::Relaxed);
        self.total_freed_bytes.fetch_add(size, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> AllocatorStatsSnapshot {
        AllocatorStatsSnapshot {
            thread_local_allocs: self.thread_local_allocs.load(Ordering::Relaxed),
            thread_local_deallocs: self.thread_local_deallocs.load(Ordering::Relaxed),
            cpu_slab_allocs: self.cpu_slab_allocs.load(Ordering::Relaxed),
            cpu_slab_deallocs: self.cpu_slab_deallocs.load(Ordering::Relaxed),
            buddy_allocs: self.buddy_allocs.load(Ordering::Relaxed),
            buddy_deallocs: self.buddy_deallocs.load(Ordering::Relaxed),
            total_allocated_bytes: self.total_allocated_bytes.load(Ordering::Relaxed),
            total_freed_bytes: self.total_freed_bytes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AllocatorStatsSnapshot {
    pub thread_local_allocs: usize,
    pub thread_local_deallocs: usize,
    pub cpu_slab_allocs: usize,
    pub cpu_slab_deallocs: usize,
    pub buddy_allocs: usize,
    pub buddy_deallocs: usize,
    pub total_allocated_bytes: usize,
    pub total_freed_bytes: usize,
}

impl AllocatorStatsSnapshot {
    pub fn active_allocations(&self) -> usize {
        let total_allocs = self.thread_local_allocs + self.cpu_slab_allocs + self.buddy_allocs;
        let total_deallocs = self.thread_local_deallocs + self.cpu_slab_deallocs + self.buddy_deallocs;
        total_allocs.saturating_sub(total_deallocs)
    }

    pub fn active_bytes(&self) -> usize {
        self.total_allocated_bytes.saturating_sub(self.total_freed_bytes)
    }

    pub fn thread_local_hit_rate(&self) -> f32 {
        let total = self.thread_local_allocs + self.cpu_slab_allocs + self.buddy_allocs;
        if total == 0 {
            0.0
        } else {
            (self.thread_local_allocs as f32 / total as f32) * 100.0
        }
    }
}

pub static ALLOCATOR_STATS: AllocatorStatistics = AllocatorStatistics::new();
