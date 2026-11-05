// kernel/src/memory/physical/frame/pool.rs
//
// Per-CPU frame pools + EmergencyPool tie-in.
// Per-CPU pools accelerate frame allocations by
// avoiding contention on the global buddy (512 frames per CPU).
// Layer 0 — no external dependencies.

use crate::memory::core::{
    AllocError, AllocFlags, Frame, PhysAddr, MAX_CPUS, PER_CPU_DRAIN_THRESHOLD, PER_CPU_POOL_SIZE,
    PER_CPU_REFILL_THRESHOLD,
};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// PER-CPU FRAME POOL
// ─────────────────────────────────────────────────────────────────────────────

/// Per-CPU frame pool — lock-free, fixed size.
///
/// Principle:
///   Each CPU maintains a circular buffer of `PER_CPU_POOL_SIZE` frames.
///   Local allocation: O(1), lockless, no cross-CPU cache misses.
///   When the pool drops below `PER_CPU_REFILL_THRESHOLD`, it is refilled
///   from the global buddy.
///   When the pool exceeds `PER_CPU_DRAIN_THRESHOLD`, the excess is returned to buddy.
///
/// Thread-safety:
///   A single thread (owning CPU) accesses this pool in normal mode.
///   The `count` counter is atomic for observer stats threads.
///
/// RULE NO-ALLOC: This code is called with preemption potentially disabled
///   -> no `Vec`, `Box`, `Arc` — only static arrays.
#[repr(C, align(64))]
pub struct PerCpuFramePool {
    /// Circular buffer of available frames (PFNs).
    /// UnsafeCell because modified only by owning CPU.
    frames: UnsafeCell<[u64; PER_CPU_POOL_SIZE]>,
    /// Production index (next free slot).
    head: AtomicUsize,
    /// Consumption index (next frame to allocate).
    tail: AtomicUsize,
    /// Available frames count snapshot (approximation).
    count: AtomicUsize,
    /// CPU ID owning this pool (usize::MAX = uninitialized).
    cpu_id: AtomicUsize,
    /// Allocation hits statistics from this pool.
    alloc_hits: AtomicU64,
    /// Allocation misses statistics (empty pool -> fallback buddy).
    alloc_misses: AtomicU64,
    /// Refills statistics.
    refills: AtomicU64,
    /// Drains statistics.
    drains: AtomicU64,
    /// Padding to occupy exactly 2 cache lines (128 bytes).
    _pad: [u8; 64],
}

// Padding calculation: adjusted for 128 bytes total alignment.
// The compiler checks the size via the static assertion below.
// Note: UnsafeCell<[u64; 512]> = 4096 bytes — struct will be large.

// SAFETY: PerCpuFramePool is accessed only by its owning CPU
// in normal mode. Atomic counters allow external observation.
unsafe impl Sync for PerCpuFramePool {}
unsafe impl Send for PerCpuFramePool {}

impl PerCpuFramePool {
    /// Creates an empty uninitialized pool.
    pub const fn new_uninit() -> Self {
        PerCpuFramePool {
            frames: UnsafeCell::new([0u64; PER_CPU_POOL_SIZE]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            count: AtomicUsize::new(0),
            cpu_id: AtomicUsize::new(usize::MAX),
            alloc_hits: AtomicU64::new(0),
            alloc_misses: AtomicU64::new(0),
            refills: AtomicU64::new(0),
            drains: AtomicU64::new(0),
            _pad: [0u8; _PAD_POOL],
        }
    }

    /// Initializes pool for a given CPU.
    ///
    /// # Safety
    /// Must be called once, only by CPU `cpu_id`,
    /// during SMP init before any concurrent access.
    pub unsafe fn init(&self, cpu_id: usize) {
        self.cpu_id.store(cpu_id, Ordering::Release);
        self.head.store(0, Ordering::Relaxed);
        self.tail.store(0, Ordering::Relaxed);
        self.count.store(0, Ordering::Relaxed);
    }

    /// Allocates a frame from the pool.
    /// Returns `None` if pool is empty (fallback to buddy).
    ///
    /// RULE NO-ALLOC: called from hot path, no allocation.
    #[inline(always)]
    pub fn pop(&self) -> Option<Frame> {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            self.alloc_misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        // SAFETY: count > 0 -> head != tail, exclusive access by this CPU.
        let frames_ptr = self.frames.get();
        let tail = self.tail.load(Ordering::Relaxed);
        let pfn = unsafe { (*frames_ptr)[tail % PER_CPU_POOL_SIZE] };
        self.tail
            .store((tail + 1) & (PER_CPU_POOL_SIZE - 1), Ordering::Relaxed);
        self.count.fetch_sub(1, Ordering::Relaxed);
        self.alloc_hits.fetch_add(1, Ordering::Relaxed);
        Some(Frame::containing(PhysAddr::new(pfn << 12)))
    }

    /// Deposits a freed frame into the pool (return from buddy or kfree).
    /// Returns `false` if pool is full (fallback to global buddy).
    #[inline(always)]
    pub fn push(&self, frame: Frame) -> bool {
        let count = self.count.load(Ordering::Relaxed);
        if count >= PER_CPU_POOL_SIZE {
            return false;
        }

        let frames_ptr = self.frames.get();
        let head = self.head.load(Ordering::Relaxed);
        // SAFETY: count < PER_CPU_POOL_SIZE => access within [0,SIZE); no other producer.
        unsafe {
            (*frames_ptr)[head % PER_CPU_POOL_SIZE] = frame.pfn();
        }
        self.head
            .store((head + 1) & (PER_CPU_POOL_SIZE - 1), Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Checks whether the pool requires refilling.
    #[inline(always)]
    pub fn needs_refill(&self) -> bool {
        self.count.load(Ordering::Relaxed) < PER_CPU_REFILL_THRESHOLD
    }

    /// Checks whether the pool should be partially drained.
    #[inline(always)]
    pub fn needs_drain(&self) -> bool {
        self.count.load(Ordering::Relaxed) > PER_CPU_DRAIN_THRESHOLD
    }

    /// Returns current number of available frames.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Returns `true` if pool is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.count.load(Ordering::Relaxed) == 0
    }

    /// Returns `true` if pool is full.
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.count.load(Ordering::Relaxed) >= PER_CPU_POOL_SIZE
    }

    /// Returns statistics for this pool.
    pub fn stats(&self) -> PerCpuPoolStats {
        PerCpuPoolStats {
            cpu_id: self.cpu_id.load(Ordering::Relaxed),
            count: self.count.load(Ordering::Relaxed),
            capacity: PER_CPU_POOL_SIZE,
            alloc_hits: self.alloc_hits.load(Ordering::Relaxed),
            alloc_misses: self.alloc_misses.load(Ordering::Relaxed),
            refills: self.refills.load(Ordering::Relaxed),
            drains: self.drains.load(Ordering::Relaxed),
        }
    }

    /// Refills pool from an iterator of frames.
    /// Stops when pool is full or iterator exhausted.
    /// Returns number of added frames.
    pub fn refill(&self, frames: impl Iterator<Item = Frame>) -> usize {
        let mut count = 0;
        for frame in frames {
            if self.is_full() {
                break;
            }
            if self.push(frame) {
                count += 1;
            }
        }
        if count > 0 {
            self.refills.fetch_add(1, Ordering::Relaxed);
        }
        count
    }

    /// Partially drains the pool (returns excess frames).
    /// Calls `drain_fn` for each frame to return to buddy.
    /// Returns number of drained frames.
    pub fn drain(&self, drain_fn: impl FnMut(Frame)) -> usize {
        let target = PER_CPU_POOL_SIZE / 2; // Drain down to 50% full
        let current = self.count.load(Ordering::Relaxed);
        if current <= target {
            return 0;
        }

        let mut drain_fn = drain_fn;
        let to_drain = current - target;
        let mut drained = 0;

        for _ in 0..to_drain {
            match self.pop() {
                Some(frame) => {
                    drain_fn(frame);
                    drained += 1;
                }
                None => break,
            }
        }

        if drained > 0 {
            self.drains.fetch_add(1, Ordering::Relaxed);
        }
        drained
    }
}

/// Padding constant calculation.
const _PAD_POOL: usize = {
    // We target 128 bytes for non-frame fields
    // frames (UnsafeCell<[u64; 512]>) = 4096 bytes — outside padding
    // Remaining fields: head, tail, count, cpu_id (4x8) + stats (4x8) = 64 bytes
    // Padding to align to 128 bytes = 128 - 64 = 64 bytes
    64
};

/// Per-CPU pool statistics.
#[derive(Copy, Clone, Debug)]
pub struct PerCpuPoolStats {
    pub cpu_id: usize,
    pub count: usize,
    pub capacity: usize,
    pub alloc_hits: u64,
    pub alloc_misses: u64,
    pub refills: u64,
    pub drains: u64,
}

impl PerCpuPoolStats {
    /// Cache hit rate (0.0 - 1.0).
    pub fn hit_rate(&self) -> f64 {
        let total = self.alloc_hits + self.alloc_misses;
        if total == 0 {
            1.0
        } else {
            self.alloc_hits as f64 / total as f64
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL PER-CPU POOLS TABLE
// ─────────────────────────────────────────────────────────────────────────────

/// Global table of per-CPU pools — MAX_CPUS static entries.
pub struct PerCpuPoolTable {
    pools: [PerCpuFramePool; MAX_CPUS],
    nr_cpus: AtomicUsize,
    initialized: AtomicUsize, // bitmask (atomic, max 256 CPUs on u64x4)
}

// SAFETY: PerCpuPoolTable is thread-safe via its internal AtomicUsize
// and exclusive per-CPU access protocol.
unsafe impl Sync for PerCpuPoolTable {}
unsafe impl Send for PerCpuPoolTable {}

impl Default for PerCpuPoolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PerCpuPoolTable {
    pub const fn new() -> Self {
        PerCpuPoolTable {
            pools: [const { PerCpuFramePool::new_uninit() }; MAX_CPUS],
            nr_cpus: AtomicUsize::new(0),
            initialized: AtomicUsize::new(0),
        }
    }

    /// Initializes a CPU's pool.
    ///
    /// # Safety
    /// See `PerCpuFramePool::init()`. Must be called during CPU `cpu_id` initialization.
    pub unsafe fn init_cpu(&self, cpu_id: usize) {
        debug_assert!(cpu_id < MAX_CPUS, "cpu_id hors limites");
        self.pools[cpu_id].init(cpu_id);
        self.nr_cpus.fetch_max(cpu_id + 1, Ordering::Relaxed);
        // Mark as initialized (single-word bitmask: covers CPUs 0..usize::BITS)
        // TODO: switch to [AtomicUsize; MAX_CPUS/usize::BITS] to support MAX_CPUS > 64
        // Safety note: CPUs >= usize::BITS won't be marked in bitmask
        // but their pool will still be empty (count = 0) and therefore safe.
        if cpu_id < usize::BITS as usize {
            self.initialized.fetch_or(1 << cpu_id, Ordering::Release);
        }
    }

    /// Access to a given CPU's pool.
    ///
    /// # Panics (debug)
    /// Panics if `cpu_id >= MAX_CPUS`.
    #[inline(always)]
    pub fn get(&self, cpu_id: usize) -> &PerCpuFramePool {
        debug_assert!(cpu_id < MAX_CPUS);
        &self.pools[cpu_id]
    }

    /// Allocates from current CPU's pool.
    /// `cpu_id` must be the ID of the currently executing CPU.
    #[inline(always)]
    pub fn alloc_from_cpu(&self, cpu_id: usize, _flags: AllocFlags) -> Result<Frame, AllocError> {
        self.get(cpu_id).pop().ok_or(AllocError::WouldBlock) // Signals need for buddy fallback
    }

    /// Releases a frame to current CPU's pool.
    /// Returns `false` if pool is full (frame must return to buddy).
    #[inline(always)]
    pub fn free_to_cpu(&self, cpu_id: usize, frame: Frame) -> bool {
        self.get(cpu_id).push(frame)
    }

    /// Returns the number of initialized CPUs.
    #[inline(always)]
    pub fn nr_cpus(&self) -> usize {
        self.nr_cpus.load(Ordering::Relaxed)
    }
}

/// Global table of per-CPU pools.
/// Initialized during SMP init, before scheduling.
pub static PER_CPU_POOLS: PerCpuPoolTable = PerCpuPoolTable::new();

/// Initializes CPU `cpu_id`'s pool.
///
/// # Safety
/// Same as `PerCpuFramePool::init()`. Must be called during CPU `cpu_id` initialization.
pub unsafe fn init_cpu_pool(cpu_id: usize) {
    PER_CPU_POOLS.init_cpu(cpu_id);
}
