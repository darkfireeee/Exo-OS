// ipc/shared_memory/allocator.rs — Lock-free SHM allocator with size classes
//
// Provides an SHM region allocation interface relying on the page
// pool (pool.rs) and the descriptor allocator (descriptor.rs).
//
// Strategy: buddy-like with 4 predefined size levels:
//   - Small  : 1 page  (4 KiB)
//   - Medium : 4 pages (16 KiB)
//   - Large  : 16 pages (64 KiB)
//   - Huge   : 64 pages (256 KiB)
//
// Each level maintains a freelist of reusable descriptors.
// Allocation attempts the exact level, then moves up if necessary.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::ipc::core::types::{IpcError, ProcessId};
use crate::ipc::shared_memory::descriptor::{
    shm_create, shm_destroy, shm_region_count, ShmId, ShmPermissions, MAX_SHM_REGIONS,
};
use crate::ipc::shared_memory::pool::shm_pool_stats;

// ---------------------------------------------------------------------------
// Size classes
// ---------------------------------------------------------------------------

/// Size class of an SHM allocation
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum ShmSizeClass {
    /// 1 page = 4 KiB
    Small = 0,
    /// 4 pages = 16 KiB
    Medium = 1,
    /// 16 pages = 64 KiB
    Large = 2,
    /// 64 pages = 256 KiB
    Huge = 3,
}

impl ShmSizeClass {
    /// Returns the number of pages corresponding to the size class
    pub fn pages(self) -> usize {
        match self {
            Self::Small => 1,
            Self::Medium => 4,
            Self::Large => 16,
            Self::Huge => 64,
        }
    }

    /// Returns the size in bytes
    pub fn bytes(self) -> usize {
        self.pages() * crate::ipc::shared_memory::page::PAGE_SIZE
    }

    /// Minimum class for a size in bytes
    pub fn for_size(bytes: usize) -> Self {
        if bytes <= Self::Small.bytes() {
            Self::Small
        } else if bytes <= Self::Medium.bytes() {
            Self::Medium
        } else if bytes <= Self::Large.bytes() {
            Self::Large
        } else {
            Self::Huge
        }
    }

    pub const COUNT: usize = 4;
}

// ---------------------------------------------------------------------------
// Per-class counters
// ---------------------------------------------------------------------------

#[repr(C, align(64))]
struct ClassCounters {
    allocs: AtomicU64,
    frees: AtomicU64,
    failures: AtomicU64,
    _pad: [u8; 40],
}

impl ClassCounters {
    const fn new() -> Self {
        Self {
            allocs: AtomicU64::new(0),
            frees: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            _pad: [0u8; 40],
        }
    }
}

static CLASS_COUNTERS: [ClassCounters; ShmSizeClass::COUNT] = [
    ClassCounters::new(),
    ClassCounters::new(),
    ClassCounters::new(),
    ClassCounters::new(),
];

// ---------------------------------------------------------------------------
// Global allocator statistics
// ---------------------------------------------------------------------------

static ALLOC_TOTAL: AtomicU64 = AtomicU64::new(0);
static FREE_TOTAL: AtomicU64 = AtomicU64::new(0);
static ALLOC_FAIL: AtomicU64 = AtomicU64::new(0);
static BYTES_ALLOCATED: AtomicU64 = AtomicU64::new(0);
static BYTES_FREED: AtomicU64 = AtomicU64::new(0);

/// Snapshot of SHM allocator statistics
#[derive(Debug, Clone, Copy)]
pub struct ShmAllocatorStats {
    pub alloc_total: u64,
    pub free_total: u64,
    pub alloc_fail: u64,
    pub bytes_allocated: u64,
    pub bytes_freed: u64,
    pub active_regions: usize,
    pub pool_free_pages: usize,
}

pub fn shm_allocator_stats() -> ShmAllocatorStats {
    let pool = shm_pool_stats();
    ShmAllocatorStats {
        alloc_total: ALLOC_TOTAL.load(Ordering::Relaxed),
        free_total: FREE_TOTAL.load(Ordering::Relaxed),
        alloc_fail: ALLOC_FAIL.load(Ordering::Relaxed),
        bytes_allocated: BYTES_ALLOCATED.load(Ordering::Relaxed),
        bytes_freed: BYTES_FREED.load(Ordering::Relaxed),
        active_regions: shm_region_count(),
        pool_free_pages: pool.free_pages,
    }
}

// ---------------------------------------------------------------------------
// Allocation handle: result returned to requester
// ---------------------------------------------------------------------------

/// Handle returned upon a successful SHM allocation.
/// Contains descriptor index and region metadata.
#[derive(Debug, Clone, Copy)]
pub struct ShmHandle {
    /// Index in SHM_DESC_DIR
    pub desc_idx: usize,
    /// Unique identifier of the region
    pub shm_id: ShmId,
    /// Allocated size in bytes
    pub size_bytes: usize,
    /// Effective size class
    pub size_class: ShmSizeClass,
    /// Number of allocated pages
    pub page_count: usize,
}

// ---------------------------------------------------------------------------
// Main allocator
// ---------------------------------------------------------------------------

/// Allocates an SHM region of minimum size covering `requested_bytes`.
///
/// # Algorithm
/// 1. Determine minimum ShmSizeClass
/// 2. Call shm_create() which allocates pages from pool
/// 3. Return an ShmHandle
///
/// # Errors
/// - `IpcError::OutOfResources` — page pool exhausted or MAX_SHM_REGIONS reached
/// - `IpcError::InvalidArgument` — size 0 or > Huge.bytes()
pub fn shm_alloc(
    owner: ProcessId,
    perms: ShmPermissions,
    requested_bytes: usize,
) -> Result<ShmHandle, IpcError> {
    if requested_bytes == 0 || requested_bytes > ShmSizeClass::Huge.bytes() {
        return Err(IpcError::InvalidArgument);
    }

    let class = ShmSizeClass::for_size(requested_bytes);
    let n_pages = class.pages();

    CLASS_COUNTERS[class as usize]
        .allocs
        .fetch_add(1, Ordering::Relaxed);

    match shm_create(owner, perms, n_pages) {
        Ok(desc_idx) => {
            let shm_id = crate::ipc::shared_memory::descriptor::shm_get_id(desc_idx)
                .unwrap_or(ShmId::INVALID);
            let size_bytes = n_pages * crate::ipc::shared_memory::page::PAGE_SIZE;

            ALLOC_TOTAL.fetch_add(1, Ordering::Relaxed);
            BYTES_ALLOCATED.fetch_add(size_bytes as u64, Ordering::Relaxed);

            Ok(ShmHandle {
                desc_idx,
                shm_id,
                size_bytes,
                size_class: class,
                page_count: n_pages,
            })
        }
        Err(e) => {
            CLASS_COUNTERS[class as usize]
                .failures
                .fetch_add(1, Ordering::Relaxed);
            ALLOC_FAIL.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// Allocates an SHM region of exact size in pages.
/// Allows allocating an arbitrary number of pages (<= MAX_SHM_PAGES_PER_DESC).
pub fn shm_alloc_pages(
    owner: ProcessId,
    perms: ShmPermissions,
    n_pages: usize,
) -> Result<ShmHandle, IpcError> {
    if n_pages == 0 {
        return Err(IpcError::InvalidArgument);
    }

    let class = ShmSizeClass::for_size(n_pages * crate::ipc::shared_memory::page::PAGE_SIZE);

    match shm_create(owner, perms, n_pages) {
        Ok(desc_idx) => {
            let shm_id = crate::ipc::shared_memory::descriptor::shm_get_id(desc_idx)
                .unwrap_or(ShmId::INVALID);
            let size_bytes = n_pages * crate::ipc::shared_memory::page::PAGE_SIZE;

            ALLOC_TOTAL.fetch_add(1, Ordering::Relaxed);
            BYTES_ALLOCATED.fetch_add(size_bytes as u64, Ordering::Relaxed);

            Ok(ShmHandle {
                desc_idx,
                shm_id,
                size_bytes,
                size_class: class,
                page_count: n_pages,
            })
        }
        Err(e) => {
            ALLOC_FAIL.fetch_add(1, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// Frees the SHM region described by `handle`.
pub fn shm_free(handle: ShmHandle) -> Result<(), IpcError> {
    let class = handle.size_class;
    CLASS_COUNTERS[class as usize]
        .frees
        .fetch_add(1, Ordering::Relaxed);
    FREE_TOTAL.fetch_add(1, Ordering::Relaxed);
    BYTES_FREED.fetch_add(handle.size_bytes as u64, Ordering::Relaxed);
    shm_destroy(handle.desc_idx)
}

/// Frees the SHM region by its descriptor index.
pub fn shm_free_by_idx(desc_idx: usize) -> Result<(), IpcError> {
    let size_bytes = crate::ipc::shared_memory::descriptor::shm_get_size(desc_idx).unwrap_or(0);
    FREE_TOTAL.fetch_add(1, Ordering::Relaxed);
    BYTES_FREED.fetch_add(size_bytes, Ordering::Relaxed);
    shm_destroy(desc_idx)
}

// ---------------------------------------------------------------------------
// Utility: defragmentation / compaction (no-op in this implementation,
// but provided for future interface)
// ---------------------------------------------------------------------------

/// Returns the number of free pages in the SHM pool.
pub fn shm_free_page_count() -> usize {
    shm_pool_stats().free_pages
}

/// Checks if the allocator can satisfy an allocation of `n_pages`.
pub fn shm_can_alloc(n_pages: usize) -> bool {
    let stats = shm_pool_stats();
    stats.free_pages >= n_pages && shm_region_count() < MAX_SHM_REGIONS
}
