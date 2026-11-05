// kernel/src/memory/heap/thread_local/cache.rs
//
// Per-CPU cache for small heap allocations.
// Each CPU maintains a magazine pair per size class.
// Alloc/free hot path is entirely lock-free.
//
// LAYER 0 — no external dependencies.

use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::core::types::AllocFlags;
use crate::memory::heap::allocator::size_classes::{heap_size_class_for, HEAP_SIZE_CLASSES};
use crate::memory::heap::thread_local::magazine::{CpuMagazinePair, MAGAZINE_SIZE};
use crate::memory::physical::allocator::slub::SLUB_CACHES;

/// Maximum number of supported CPUs.
pub use crate::memory::core::constants::MAX_CPUS;

/// Compile-time assertion.
const _: () = assert!(
    MAX_CPUS == crate::memory::core::constants::MAX_CPUS,
    "heap cache MAX_CPUS doit correspondre à memory::core::constants::MAX_CPUS"
);

// ─────────────────────────────────────────────────────────────────────────────
// PER-CPU STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

/// Detailed statistics per CPU and per size class.
pub struct PerCpuCacheStats {
    pub hits: AtomicU64,      // allocs served by local cache
    pub misses: AtomicU64,    // allocs that had to go to SLUB
    pub free_hits: AtomicU64, // frees absorbed by local cache
    pub free_miss: AtomicU64, // frees returned to SLUB
    pub drains: AtomicU64,    // number of complete drains (context switch, etc.)
    pub refills: AtomicU64,   // number of refills from SLUB
}

impl PerCpuCacheStats {
    const fn new() -> Self {
        PerCpuCacheStats {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            free_hits: AtomicU64::new(0),
            free_miss: AtomicU64::new(0),
            drains: AtomicU64::new(0),
            refills: AtomicU64::new(0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PER-CPU CACHE (MAIN STRUCTURE)
// ─────────────────────────────────────────────────────────────────────────────

/// Number of size classes covered by the per-CPU cache.
/// Covers only classes <= 2048 bytes (SLUB path).
pub const CACHED_SIZE_CLASSES: usize = 16;

/// Per-CPU cache = array of magazine pairs, one per size class.
///
/// Aligned to 64 bytes (avoids false-sharing between CPUs in an array).
#[repr(C, align(64))]
pub struct PerCpuCache {
    /// Magazine pairs for each size class.
    pub magazines: [CpuMagazinePair; CACHED_SIZE_CLASSES],
    /// Owning CPU of this cache.
    cpu_id: u32,
    /// Is cache active? (false as long as uninitialized).
    pub active: bool,
    /// Statistics.
    pub stats: PerCpuCacheStats,
    /// Padding to align to 64 * N.
    _pad: [u8; 3],
}

impl PerCpuCache {
    /// Builds an empty cache (without runtime initialization).
    pub const fn new_uninit() -> Self {
        // We generate the 16 magazine pairs at construction.
        // Rust const fn does not yet allow loops on [T; N] on stable,
        // so we initialize explicitly.
        PerCpuCache {
            magazines: [
                CpuMagazinePair::new(0),
                CpuMagazinePair::new(1),
                CpuMagazinePair::new(2),
                CpuMagazinePair::new(3),
                CpuMagazinePair::new(4),
                CpuMagazinePair::new(5),
                CpuMagazinePair::new(6),
                CpuMagazinePair::new(7),
                CpuMagazinePair::new(8),
                CpuMagazinePair::new(9),
                CpuMagazinePair::new(10),
                CpuMagazinePair::new(11),
                CpuMagazinePair::new(12),
                CpuMagazinePair::new(13),
                CpuMagazinePair::new(14),
                CpuMagazinePair::new(15),
            ],
            cpu_id: 0,
            active: false,
            stats: PerCpuCacheStats::new(),
            _pad: [0u8; 3],
        }
    }

    /// Initializes the cache for a given CPU.
    pub fn init(&mut self, cpu_id: u32) {
        self.cpu_id = cpu_id;
        self.active = true;
        for (i, mag) in self.magazines.iter_mut().enumerate() {
            mag.loaded.size_class = i;
            mag.prev.size_class = i;
        }
    }

    /// Allocates an object of `size` bytes from the per-CPU cache.
    ///
    /// Returns `None` if the cache is exhausted for this class
    /// (caller must then invoke `alloc_slow`).
    #[inline]
    pub fn alloc_fast(&mut self, size: usize) -> Option<NonNull<u8>> {
        if !self.active {
            return None;
        }
        let class_idx = heap_size_class_for(size)?;
        if class_idx >= CACHED_SIZE_CLASSES {
            return None;
        }

        if let Some(ptr) = self.magazines[class_idx].alloc() {
            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            Some(ptr)
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Frees `ptr` (of size `size`) into the per-CPU cache.
    ///
    /// Returns `false` if the cache is saturated (caller must drain).
    #[inline]
    pub fn free_fast(&mut self, ptr: NonNull<u8>, size: usize) -> bool {
        if !self.active {
            return false;
        }
        let class_idx = match heap_size_class_for(size) {
            Some(idx) => idx,
            None => return false,
        };
        if class_idx >= CACHED_SIZE_CLASSES {
            return false;
        }

        if self.magazines[class_idx].free(ptr) {
            self.stats.free_hits.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            self.stats.free_miss.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Slow path: recharges magazine from SLUB then serves the allocation.
    pub fn alloc_slow(&mut self, _size: usize, class_idx: usize) -> Option<NonNull<u8>> {
        // Fill loaded magazine from SLUB (batch of MAGAZINE_SIZE/2).
        let batch = MAGAZINE_SIZE / 2;
        let slub_idx = HEAP_SIZE_CLASSES[class_idx].slab_idx;
        let mut loaded = 0usize;

        for _ in 0..batch {
            match SLUB_CACHES[slub_idx].alloc(AllocFlags::NONE) {
                Ok(ptr) => {
                    // Push directly into prev magazine (loaded is full on alloc side).
                    if !self.magazines[class_idx].prev.push(ptr) {
                        break;
                    }
                    loaded += 1;
                }
                Err(_) => break,
            }
        }

        if loaded == 0 {
            return None;
        }

        // Swap loaded <-> prev to put the filled prev into "loaded" position.
        core::mem::swap(
            &mut self.magazines[class_idx].loaded,
            &mut self.magazines[class_idx].prev,
        );

        self.stats.refills.fetch_add(1, Ordering::Relaxed);
        self.magazines[class_idx].loaded.pop()
    }

    /// Drains the loaded magazine to SLUB (called during context switch
    /// or when magazine is full on the slow path).
    pub fn drain_class(&mut self, class_idx: usize) {
        if class_idx >= CACHED_SIZE_CLASSES {
            return;
        }
        let slub_idx = HEAP_SIZE_CLASSES[class_idx].slab_idx;

        // Drain loaded.
        while let Some(ptr) = self.magazines[class_idx].loaded.pop() {
            // SAFETY: ptr was allocated by the corresponding SLUB.
            unsafe {
                SLUB_CACHES[slub_idx].free(ptr);
            }
        }
        // Drain prev.
        while let Some(ptr) = self.magazines[class_idx].prev.pop() {
            // SAFETY: ptr allocated by SLUB_CACHES[slub_idx] (prev magazine).
            unsafe {
                SLUB_CACHES[slub_idx].free(ptr);
            }
        }
        self.stats.drains.fetch_add(1, Ordering::Relaxed);
    }

    /// Drains all classes (full context switch).
    pub fn drain_all(&mut self) {
        for i in 0..CACHED_SIZE_CLASSES {
            self.drain_class(i);
        }
    }

    /// Total number of cached objects on this CPU.
    pub fn total_cached(&self) -> usize {
        self.magazines.iter().map(|m| m.cached_count()).sum()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL PER-CPU CACHE TABLE
// ─────────────────────────────────────────────────────────────────────────────

/// Global table: one cache per CPU.
/// Access is protected because each CPU only accesses its own cache.
/// A `Mutex<()>` is used only for initialization (one-time).
#[allow(dead_code)]
pub struct PerCpuCacheTable {
    caches: [PerCpuCache; MAX_CPUS],
    init_lock: spin::Mutex<bool>,
}

// SAFETY: Access to `caches[cpu_id]` is exclusive to cpu_id (no inter-CPU sharing).
unsafe impl Sync for PerCpuCacheTable {}
unsafe impl Send for PerCpuCacheTable {}

impl PerCpuCacheTable {
    const fn new() -> Self {
        // Initialize the MAX_CPUS caches statically.
        // Rust does not yet allow `[expr; MAX_CPUS]` with complex non-Copy types,
        // so we use a repetitive macro/transmute approach.
        PerCpuCacheTable {
            caches: {
                // SAFETY: The structure is entirely initialized by const fn.
                // We initially zero all fields via transmute since [u8] is
                // not available in const. We use a copy of new_uninit approach.
                //
                // In stable Rust, we cannot do [PerCpuCache::new_uninit(); 256] if PerCpuCache
                // is not Copy. We initialize via MaybeUninit during runtime init.
                // To bypass this in const, we encode the MAX_CPUS entries explicitly
                // by delegating to a helper that returns the array.
                //
                // Trick: const { ... } + unsafe transmute of a zeroed array.
                // SAFETY: All AtomicXxx and primitives cleanly initialize with 0.
                // `active: false` = 0u8 ok, `cpu_id: 0` ok, all AtomicU64 = 0 ok.
                // @rustc: `core::mem::MaybeUninit::zeroed().assume_init()` is not const stable.
                // We therefore use runtime initialization deferred to `init_cpu()`.
                //
                // SAFETY: valid zeroes for AtomicXxx and primitives; init_cpu() overwrites afterward.
                unsafe { core::mem::MaybeUninit::zeroed().assume_init() }
            },
            init_lock: spin::Mutex::new(false),
        }
    }

    /// Initializes cache for a given CPU. Must be called once per CPU.
    ///
    /// # Safety
    /// Called from CPU initialization code before any allocation.
    pub unsafe fn init_cpu(&self, cpu_id: usize) {
        assert!(
            cpu_id < MAX_CPUS,
            "vmalloc: cpu_id {} >= MAX_CPUS {}",
            cpu_id,
            MAX_CPUS
        );
        // SAFETY: each CPU accesses only its own slot.
        let cache_ptr = &self.caches[cpu_id] as *const PerCpuCache as *mut PerCpuCache;
        (*cache_ptr).init(cpu_id as u32);
    }

    /// Mutable access to current CPU's cache.
    ///
    /// # Safety
    /// `cpu_id` must match current CPU. No preemption during access.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self, cpu_id: usize) -> &mut PerCpuCache {
        let cache_ptr = core::ptr::addr_of!(self.caches[cpu_id]) as *mut PerCpuCache;
        &mut *cache_ptr
    }
}

/// Global table of per-CPU caches.
pub static CPU_CACHES: PerCpuCacheTable = PerCpuCacheTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// SIMPLIFIED PUBLIC API
// ─────────────────────────────────────────────────────────────────────────────

/// Full allocation path with per-CPU cache.
///
/// 1. Tries cache (fast path — lock-free).
/// 2. If missed, refill from SLUB (slow path).
/// 3. If still nothing, bubbles up OOM error.
///
/// # Safety
/// `cpu_id` must be current CPU, without preemption during call.
pub unsafe fn cache_alloc(size: usize, cpu_id: usize) -> Option<NonNull<u8>> {
    let cache = CPU_CACHES.get_mut(cpu_id);
    if let Some(ptr) = cache.alloc_fast(size) {
        return Some(ptr);
    }

    // Slow path: refill.
    let class_idx = heap_size_class_for(size)?;
    cache.alloc_slow(size, class_idx)
}

/// Full free path with per-CPU cache.
///
/// 1. Tries to store in cache (fast path).
/// 2. If full, drains half to SLUB, then retries.
///
/// # Safety
/// `cpu_id` must be current CPU, without preemption during call.
pub unsafe fn cache_free(ptr: NonNull<u8>, size: usize, cpu_id: usize) {
    let cache = CPU_CACHES.get_mut(cpu_id);
    if cache.free_fast(ptr, size) {
        return;
    }

    // Slow path: drain relevant class then free directly via SLUB.
    if let Some(class_idx) = heap_size_class_for(size) {
        if class_idx < CACHED_SIZE_CLASSES {
            cache.drain_class(class_idx);
            // After drain, retry.
            if cache.free_fast(ptr, size) {
                return;
            }
        }
    }
    // Last resort: free directly via SLUB.
    if let Some(class_idx) = heap_size_class_for(size) {
        let slub_idx = HEAP_SIZE_CLASSES[class_idx].slab_idx;
        SLUB_CACHES[slub_idx].free(ptr);
    }
}
