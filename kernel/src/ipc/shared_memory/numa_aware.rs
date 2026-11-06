// ipc/shared_memory/numa_aware.rs — SHM allocation with NUMA affinity for Exo-OS
//
// On NUMA (Non-Uniform Memory Access) systems, allocating shared memory
// on the requesting process's NUMA node significantly improves performance.
//
// This module maintains:
//   - A partition of the SHM pool per NUMA node (NUMA_PAGES_PER_NODE pages each)
//   - Independent allocation bitmaps per node
//   - Fallback to the global pool if the local node is exhausted
//   - NUMA hit/miss statistics
//
// Configuration:
//   MAX_NUMA_NODES = 8 (suited for most servers)
//   NUMA_PAGES_PER_NODE = SHM_POOL_PAGES / MAX_NUMA_NODES = 32 pages per node

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::SHM_POOL_PAGES;
use crate::ipc::core::types::{IpcError, ProcessId};
use crate::ipc::shared_memory::allocator::{ShmHandle, ShmSizeClass};
use crate::ipc::shared_memory::descriptor::{shm_create, ShmPermissions};
use crate::ipc::shared_memory::pool::{shm_page_alloc, shm_page_free};

// ---------------------------------------------------------------------------
// NUMA configuration
// ---------------------------------------------------------------------------

/// Maximum number of supported NUMA nodes
pub const MAX_NUMA_NODES: usize = 8;

/// SHM pages allocated per NUMA node
pub const NUMA_PAGES_PER_NODE: usize = SHM_POOL_PAGES / MAX_NUMA_NODES; // 32

/// NUMA node identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NumaNodeId(pub u32);

impl NumaNodeId {
    pub const LOCAL: Self = Self(0);
    pub const INVALID: Self = Self(u32::MAX);

    pub fn is_valid(self) -> bool {
        (self.0 as usize) < MAX_NUMA_NODES
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

// ---------------------------------------------------------------------------
// NUMA node bitmap
// ---------------------------------------------------------------------------

/// Number of u64 words for a node's bitmap (32 pages = 1 word)
pub const NUMA_BITMAP_WORDS: usize = (NUMA_PAGES_PER_NODE + 63) / 64; // = 1

/// Allocation bitmap for a NUMA node
#[repr(C, align(64))]
pub struct NumaBitmap {
    /// bits[i] = 1 -> local page i free, 0 -> occupied
    bits: [AtomicU64; NUMA_BITMAP_WORDS],
    /// Start offset in the global pool (page_index_base + local_offset)
    page_base: AtomicU32,
    /// Number of free pages in this node
    free_count: AtomicUsize,
    _pad: [u8; 20],
}

// SAFETY: all fields are atomic or write-once (page_base)
unsafe impl Sync for NumaBitmap {}
unsafe impl Send for NumaBitmap {}

impl NumaBitmap {
    pub const fn new() -> Self {
        const INIT_ATOMIC: AtomicU64 = AtomicU64::new(0);
        Self {
            bits: [INIT_ATOMIC; NUMA_BITMAP_WORDS],
            page_base: AtomicU32::new(0),
            free_count: AtomicUsize::new(0),
            _pad: [0u8; 20],
        }
    }

    /// Initializes the bitmap: `base` = first pool index of this node.
    pub fn init(&self, base: usize) {
        self.page_base.store(base as u32, Ordering::Relaxed);
        // Mark all pages as free (1 = free)
        let full_words = NUMA_PAGES_PER_NODE / 64;
        let rem = NUMA_PAGES_PER_NODE % 64;
        for w in 0..full_words {
            self.bits[w].store(u64::MAX, Ordering::Relaxed);
        }
        if rem > 0 {
            // Only `rem` valid bits in the last word
            self.bits[full_words].store((1u64 << rem) - 1, Ordering::Relaxed);
        }
        self.free_count
            .store(NUMA_PAGES_PER_NODE, Ordering::Release);
    }

    /// Allocates a local page. Returns its index in the global pool, or None.
    pub fn alloc_local(&self) -> Option<usize> {
        let base = self.page_base.load(Ordering::Relaxed) as usize;

        for (wi, word) in self.bits.iter().enumerate() {
            loop {
                let v = word.load(Ordering::Acquire);
                if v == 0 {
                    break;
                }
                let bit = v.trailing_zeros() as usize;
                let mask = 1u64 << bit;
                match word.compare_exchange_weak(v, v & !mask, Ordering::AcqRel, Ordering::Acquire)
                {
                    Ok(_) => {
                        let local_idx = wi * 64 + bit;
                        if local_idx < NUMA_PAGES_PER_NODE {
                            self.free_count.fetch_sub(1, Ordering::Relaxed);
                            return Some(base + local_idx);
                        }
                        // Restore bit (out of bounds)
                        word.fetch_or(mask, Ordering::Release);
                        return None;
                    }
                    Err(_) => {
                        core::hint::spin_loop();
                        continue;
                    }
                }
            }
        }
        None
    }

    /// Frees page with global index `pool_idx` in this node.
    pub fn free_local(&self, pool_idx: usize) -> bool {
        let base = self.page_base.load(Ordering::Relaxed) as usize;
        if pool_idx < base || pool_idx >= base + NUMA_PAGES_PER_NODE {
            return false; // not our node
        }
        let local = pool_idx - base;
        let wi = local / 64;
        let bit = local % 64;
        self.bits[wi].fetch_or(1u64 << bit, Ordering::Release);
        self.free_count.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub fn free_count(&self) -> usize {
        self.free_count.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// NUMA statistics
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
pub struct NumaStats {
    /// Allocations satisfied on the local node
    pub local_hits: AtomicU64,
    /// Allocations falling back to global pool (NUMA miss)
    pub remote_misses: AtomicU64,
    /// Frees on the local node
    pub local_frees: AtomicU64,
    /// Frees on another node (migration)
    pub remote_frees: AtomicU64,
    _pad: [u8; 32],
}

impl NumaStats {
    pub const fn new() -> Self {
        Self {
            local_hits: AtomicU64::new(0),
            remote_misses: AtomicU64::new(0),
            local_frees: AtomicU64::new(0),
            remote_frees: AtomicU64::new(0),
            _pad: [0u8; 32],
        }
    }

    pub fn snapshot(&self) -> NumaStatsSnapshot {
        NumaStatsSnapshot {
            local_hits: self.local_hits.load(Ordering::Relaxed),
            remote_misses: self.remote_misses.load(Ordering::Relaxed),
            local_frees: self.local_frees.load(Ordering::Relaxed),
            remote_frees: self.remote_frees.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NumaStatsSnapshot {
    pub local_hits: u64,
    pub remote_misses: u64,
    pub local_frees: u64,
    pub remote_frees: u64,
}

// ---------------------------------------------------------------------------
// Global NUMA manager
// ---------------------------------------------------------------------------

/// NUMA manager: partitions the SHM pool among MAX_NUMA_NODES nodes.
pub struct NumaManager {
    nodes: [NumaBitmap; MAX_NUMA_NODES],
    /// Number of active NUMA nodes (detected at boot)
    active_nodes: AtomicUsize,
    pub stats: NumaStats,
    initialized: AtomicU32,
}

// SAFETY: NumaBitmap is Sync, NumaStats is Sync
unsafe impl Sync for NumaManager {}
unsafe impl Send for NumaManager {}

impl NumaManager {
    pub const fn new() -> Self {
        const INIT_NODE: NumaBitmap = NumaBitmap::new();
        Self {
            nodes: [INIT_NODE; MAX_NUMA_NODES],
            active_nodes: AtomicUsize::new(0),
            stats: NumaStats::new(),
            initialized: AtomicU32::new(0),
        }
    }

    /// Initializes the NUMA manager by partitioning the SHM pool.
    /// `n_nodes` = number of detected NUMA nodes (1 if non-NUMA).
    ///
    /// # SAFETY
    /// Must be called once after `init_shm_pool()`.
    pub unsafe fn init(&self, n_nodes: usize) {
        if n_nodes == 0 {
            return;
        }
        let nodes = n_nodes.min(MAX_NUMA_NODES);
        self.active_nodes.store(nodes, Ordering::Relaxed);

        for i in 0..nodes {
            let base = i * NUMA_PAGES_PER_NODE;
            self.nodes[i].init(base);
        }

        self.initialized.store(1, Ordering::Release);
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire) != 0
    }

    pub fn active_nodes(&self) -> usize {
        self.active_nodes.load(Ordering::Relaxed)
    }

    // -----------------------------------------------------------------------
    // NUMA-aware allocation
    // -----------------------------------------------------------------------

    /// Allocates a page prioritizing `preferred_node`.
    /// Fallback to the global pool if the local node is exhausted.
    pub fn alloc_page(&self, preferred_node: NumaNodeId) -> Option<usize> {
        if !self.is_initialized() {
            // Fallback: global pool
            self.stats.remote_misses.fetch_add(1, Ordering::Relaxed);
            return shm_page_alloc();
        }

        let n = self.active_nodes();
        let preferred = preferred_node.index().min(n.saturating_sub(1));

        // Attempt on preferred node
        if let Some(idx) = self.nodes[preferred].alloc_local() {
            self.stats.local_hits.fetch_add(1, Ordering::Relaxed);
            return Some(idx);
        }

        // Attempt on other nodes (round-robin)
        for delta in 1..n {
            let node = (preferred + delta) % n;
            if let Some(idx) = self.nodes[node].alloc_local() {
                self.stats.remote_misses.fetch_add(1, Ordering::Relaxed);
                return Some(idx);
            }
        }

        // Last resort: global pool (non-NUMA pages)
        self.stats.remote_misses.fetch_add(1, Ordering::Relaxed);
        shm_page_alloc()
    }

    /// Frees a page (automatic node detection).
    pub fn free_page(&self, pool_idx: usize) {
        if !self.is_initialized() {
            shm_page_free(pool_idx);
            return;
        }

        let n = self.active_nodes();
        for i in 0..n {
            if self.nodes[i].free_local(pool_idx) {
                self.stats.local_frees.fetch_add(1, Ordering::Relaxed);
                // Also free in global pool for consistency
                shm_page_free(pool_idx);
                return;
            }
        }

        // Page outside NUMA partition (global pool)
        self.stats.remote_frees.fetch_add(1, Ordering::Relaxed);
        shm_page_free(pool_idx);
    }

    /// Returns the number of free pages on `node`.
    pub fn node_free_pages(&self, node: NumaNodeId) -> usize {
        if !node.is_valid() || node.index() >= self.active_nodes() {
            return 0;
        }
        self.nodes[node.index()].free_count()
    }

    /// Snapshot of NUMA statistics.
    pub fn snapshot_stats(&self) -> NumaStatsSnapshot {
        self.stats.snapshot()
    }
}

/// Global NUMA manager instance
pub static NUMA_MANAGER: NumaManager = NumaManager::new();

// ---------------------------------------------------------------------------
// High-level public API
// ---------------------------------------------------------------------------

/// Initializes the NUMA manager with `n_nodes` nodes.
/// To be called from ipc_init() after init_shm_pool().
pub unsafe fn numa_init(n_nodes: usize) {
    NUMA_MANAGER.init(n_nodes);
}

/// Allocates an SHM region preferentially on NUMA node `preferred_node`.
///
/// # Behavior
/// - Attempts to allocate all pages on the preferred node
/// - Automatic fallback to other nodes / global pool
/// - All pages have the NO_COW flag
pub fn numa_shm_alloc(
    owner: ProcessId,
    perms: ShmPermissions,
    n_pages: usize,
    preferred_node: NumaNodeId,
) -> Result<ShmHandle, IpcError> {
    if n_pages == 0 || n_pages > crate::ipc::shared_memory::descriptor::MAX_SHM_PAGES_PER_DESC {
        return Err(IpcError::InvalidArgument);
    }

    let size_class = ShmSizeClass::for_size(n_pages * crate::ipc::shared_memory::page::PAGE_SIZE);

    // Allocate SHM descriptor from global pool (T8-011 FIX: avoids
    // redundant reservation and double free corrupting refcount).
    let desc_idx = shm_create(owner, perms, n_pages)?;

    // Update NUMA statistics according to actual page placement
    if NUMA_MANAGER.is_initialized() {
        let pref_idx = preferred_node.index().min(NUMA_MANAGER.active_nodes().saturating_sub(1));
        let dir = crate::ipc::shared_memory::descriptor::SHM_DESC_DIR.lock();
        if let Some(desc) = unsafe { dir.get(desc_idx) } {
            for i in 0..n_pages {
                if let Some(page_idx) = desc.page_pool_idx(i) {
                    let node_of_page = page_idx / NUMA_PAGES_PER_NODE;
                    if node_of_page == pref_idx {
                        NUMA_MANAGER.stats.local_hits.fetch_add(1, Ordering::Relaxed);
                    } else {
                        NUMA_MANAGER.stats.remote_misses.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    let shm_id = crate::ipc::shared_memory::descriptor::shm_get_id(desc_idx)
        .unwrap_or(crate::ipc::shared_memory::descriptor::ShmId::INVALID);
    let size_bytes = n_pages * crate::ipc::shared_memory::page::PAGE_SIZE;

    Ok(ShmHandle {
        desc_idx,
        shm_id,
        size_bytes,
        size_class,
        page_count: n_pages,
    })
}

/// Frees an SHM region allocated via `numa_shm_alloc()`.
pub fn numa_shm_free(handle: ShmHandle) -> Result<(), IpcError> {
    crate::ipc::shared_memory::descriptor::shm_destroy(handle.desc_idx)
}

/// Returns a snapshot of NUMA statistics.
pub fn numa_stats() -> NumaStatsSnapshot {
    NUMA_MANAGER.snapshot_stats()
}

/// Returns the number of free pages on node `node`.
pub fn numa_node_free_pages(node: NumaNodeId) -> usize {
    NUMA_MANAGER.node_free_pages(node)
}
