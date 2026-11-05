// kernel/src/memory/huge_pages/hugetlbfs.rs
//
// Hugetlbfs — pool of pre-reserved 1 GiB huge pages.
// Layer 0 — no external dependencies except `spin`.
//
// Principle:
//   A static pool of HUGETLB_MAX_POOL huge pages (configurable) is
//   reserved at boot. Allocations come from this pool — unlike
//   THP which uses buddy dynamically.
//   This guarantees huge pages availability even under memory pressure.
//
// Supported sizes:
//   2 MiB (order 9)  — interoperable with thp.rs
//   1 GiB (order 18) — gigantic pages (recent processors)

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::{AllocFlags, Frame, PhysAddr, PAGE_SIZE};
use crate::memory::physical::allocator::buddy::{alloc_pages, free_pages};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Buddy order for a 1 GiB huge page (1 GiB / 4 KiB = 262144 = 2^18).
pub const GIGA_PAGE_ORDER: u32 = 18;

/// Maximum capacity of static hugetlb pool.
pub const HUGETLB_MAX_POOL: usize = 512;

// ─────────────────────────────────────────────────────────────────────────────
// POOL SLOT
// ─────────────────────────────────────────────────────────────────────────────

/// Huge page size in the pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HugeTlbSize {
    /// 2 MiB page (order 9).
    TwoMiB = 9,
    /// 1 GiB page (order 18).
    OneGiB = 18,
}

impl HugeTlbSize {
    #[inline]
    pub fn order(self) -> usize {
        self as usize
    }
    #[inline]
    pub fn size_bytes(self) -> usize {
        PAGE_SIZE << (self as usize)
    }
}

/// Slot in the hugetlb pool.
#[derive(Clone, Copy)]
struct HugeTlbSlot {
    /// Physical address of frame (0 if free).
    phys: u64,
    /// Size (buddy order).
    order: u8,
    /// Slot occupied (true = allocated to a caller).
    occupied: bool,
}

impl HugeTlbSlot {
    const EMPTY: HugeTlbSlot = HugeTlbSlot {
        phys: 0,
        order: 0,
        occupied: false,
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// HUGETLB STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

/// Statistics of hugetlb pool.
pub struct HugeTlbStats {
    /// Number of slots in the pool (configurable size at boot).
    pub pool_size: AtomicU32,
    /// Number of slots currently allocated.
    pub in_use: AtomicU32,
    /// Total number of successful allocations.
    pub alloc_success: AtomicU64,
    /// Number of failed allocations (pool exhausted).
    pub alloc_fail: AtomicU64,
    /// Total number of frees.
    pub frees: AtomicU64,
}

impl Default for HugeTlbStats {
    fn default() -> Self {
        Self::new()
    }
}

impl HugeTlbStats {
    const fn new() -> Self {
        HugeTlbStats {
            pool_size: AtomicU32::new(0),
            in_use: AtomicU32::new(0),
            alloc_success: AtomicU64::new(0),
            alloc_fail: AtomicU64::new(0),
            frees: AtomicU64::new(0),
        }
    }
}

pub static HUGETLB_STATS: HugeTlbStats = HugeTlbStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// HUGETLB POOL
// ─────────────────────────────────────────────────────────────────────────────

struct HugeTlbPoolInner {
    slots: [HugeTlbSlot; HUGETLB_MAX_POOL],
    count: usize, // number of initialized slots
}

impl HugeTlbPoolInner {
    const fn new() -> Self {
        HugeTlbPoolInner {
            slots: [HugeTlbSlot::EMPTY; HUGETLB_MAX_POOL],
            count: 0,
        }
    }
}

/// Global hugetlb pool.
pub struct HugeTlbPool {
    inner: Mutex<HugeTlbPoolInner>,
    initialized: AtomicBool,
}

impl Default for HugeTlbPool {
    fn default() -> Self {
        Self::new()
    }
}

impl HugeTlbPool {
    const fn new() -> Self {
        HugeTlbPool {
            inner: Mutex::new(HugeTlbPoolInner::new()),
            initialized: AtomicBool::new(false),
        }
    }

    /// Initializes pool by pre-allocating `n_2mib` 2 MiB pages and `n_1gib` 1 GiB pages.
    ///
    /// # Safety
    /// CPL 0, called once at boot.
    pub unsafe fn init(&self, n_2mib: usize, n_1gib: usize) {
        if self.initialized.swap(true, Ordering::AcqRel) {
            return;
        }
        let total = (n_2mib + n_1gib).min(HUGETLB_MAX_POOL);
        let mut inner = self.inner.lock();

        // Pre-allocate 2 MiB pages.
        let alloc_2mib = n_2mib.min(total);
        for _ in 0..alloc_2mib {
            if inner.count >= HUGETLB_MAX_POOL {
                break;
            }
            match alloc_pages(HugeTlbSize::TwoMiB.order(), AllocFlags::MOVABLE) {
                Ok(frame) => {
                    let idx = inner.count;
                    inner.slots[idx] = HugeTlbSlot {
                        phys: frame.start_address().as_u64(),
                        order: HugeTlbSize::TwoMiB as u8,
                        occupied: false,
                    };
                    inner.count += 1;
                }
                Err(_) => break, // insufficient memory — stop
            }
        }

        // Pre-allocate 1 GiB pages.
        let alloc_1gib = n_1gib.min(HUGETLB_MAX_POOL - inner.count);
        for _ in 0..alloc_1gib {
            if inner.count >= HUGETLB_MAX_POOL {
                break;
            }
            match alloc_pages(HugeTlbSize::OneGiB.order(), AllocFlags::NONE) {
                Ok(frame) => {
                    let idx = inner.count;
                    inner.slots[idx] = HugeTlbSlot {
                        phys: frame.start_address().as_u64(),
                        order: HugeTlbSize::OneGiB as u8,
                        occupied: false,
                    };
                    inner.count += 1;
                }
                Err(_) => break,
            }
        }

        HUGETLB_STATS
            .pool_size
            .store(inner.count as u32, Ordering::Relaxed);
    }

    /// Allocates a huge page from the pool.
    ///
    /// Prefers free pages matching `size`. If the pool is exhausted
    /// for requested size, returns `None` (never buddy fallback from
    /// hugetlb — that is the caller's responsibility).
    pub fn alloc(&self, size: HugeTlbSize) -> Option<Frame> {
        if !self.initialized.load(Ordering::Acquire) {
            return None;
        }
        let mut inner = self.inner.lock();
        let target_order = size as u8;
        let count = inner.count;

        for slot in inner.slots[..count].iter_mut() {
            if slot.order == target_order && !slot.occupied && slot.phys != 0 {
                slot.occupied = true;
                let frame = Frame::from_phys_addr(PhysAddr::new(slot.phys));
                drop(inner);
                HUGETLB_STATS.in_use.fetch_add(1, Ordering::Relaxed);
                HUGETLB_STATS.alloc_success.fetch_add(1, Ordering::Relaxed);
                return Some(frame);
            }
        }

        HUGETLB_STATS.alloc_fail.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Frees a huge page back to the pool.
    ///
    /// # Safety
    /// `frame` must have been allocated via `HugeTlbPool::alloc`.
    pub unsafe fn free(&self, frame: Frame, size: HugeTlbSize) {
        let phys = frame.start_address().as_u64();
        let mut inner = self.inner.lock();
        let count = inner.count;

        for slot in inner.slots[..count].iter_mut() {
            if slot.phys == phys && slot.order == size as u8 && slot.occupied {
                slot.occupied = false;
                drop(inner);
                HUGETLB_STATS.in_use.fetch_sub(1, Ordering::Relaxed);
                HUGETLB_STATS.frees.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // Frame not found in pool — free into buddy (should not happen).
        let _ = free_pages(frame, size.order());
        HUGETLB_STATS.frees.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns number of free slots for the given size.
    pub fn free_count(&self, size: HugeTlbSize) -> usize {
        if !self.initialized.load(Ordering::Acquire) {
            return 0;
        }
        let inner = self.inner.lock();
        inner.slots[..inner.count]
            .iter()
            .filter(|s| s.order == size as u8 && !s.occupied && s.phys != 0)
            .count()
    }

    /// Dynamically resizes the pool.
    /// Allocates or frees slots to reach `target_count` for `size`.
    ///
    /// # Safety
    /// CPL 0.
    pub unsafe fn resize(&self, size: HugeTlbSize, target_count: usize) {
        let current_free = self.free_count(size);
        if current_free == target_count {
            return;
        }

        if current_free < target_count {
            // Enlarge: allocate additional pages.
            let to_alloc = target_count - current_free;
            let mut inner = self.inner.lock();
            for _ in 0..to_alloc {
                if inner.count >= HUGETLB_MAX_POOL {
                    break;
                }
                match alloc_pages(size.order(), AllocFlags::MOVABLE) {
                    Ok(frame) => {
                        let idx = inner.count;
                        inner.slots[idx] = HugeTlbSlot {
                            phys: frame.start_address().as_u64(),
                            order: size as u8,
                            occupied: false,
                        };
                        inner.count += 1;
                        HUGETLB_STATS.pool_size.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => break,
                }
            }
        } else {
            // Shrink: free unused pages.
            let to_free = current_free - target_count;
            let mut freed = 0usize;
            let mut inner = self.inner.lock();
            let count = inner.count;
            for slot in inner.slots[..count].iter_mut() {
                if freed >= to_free {
                    break;
                }
                if slot.order == size as u8 && !slot.occupied && slot.phys != 0 {
                    let frame = Frame::from_phys_addr(PhysAddr::new(slot.phys));
                    let _ = free_pages(frame, size.order());
                    slot.phys = 0;
                    slot.occupied = false;
                    slot.order = 0;
                    HUGETLB_STATS.pool_size.fetch_sub(1, Ordering::Relaxed);
                    freed += 1;
                }
            }
        }
    }
}

/// Global instance of hugetlb pool.
pub static HUGETLB_POOL: HugeTlbPool = HugeTlbPool::new();

// ─────────────────────────────────────────────────────────────────────────────
// PUBLIC API
// ─────────────────────────────────────────────────────────────────────────────

/// Allocates a 2 MiB huge page from hugetlb pool.
#[inline]
pub fn hugetlb_alloc_2mib() -> Option<Frame> {
    HUGETLB_POOL.alloc(HugeTlbSize::TwoMiB)
}

/// Allocates a 1 GiB huge page from hugetlb pool.
#[inline]
pub fn hugetlb_alloc_1gib() -> Option<Frame> {
    HUGETLB_POOL.alloc(HugeTlbSize::OneGiB)
}

/// Frees a 2 MiB huge page back to the pool.
///
/// # Safety
/// `frame` must have been allocated via `hugetlb_alloc_2mib`.
#[inline]
pub unsafe fn hugetlb_free_2mib(frame: Frame) {
    HUGETLB_POOL.free(frame, HugeTlbSize::TwoMiB);
}

/// Frees a 1 GiB huge page back to the pool.
///
/// # Safety
/// `frame` must have been allocated via `hugetlb_alloc_1gib`.
#[inline]
pub unsafe fn hugetlb_free_1gib(frame: Frame) {
    HUGETLB_POOL.free(frame, HugeTlbSize::OneGiB);
}

/// Returns number of available 2 MiB huge pages in pool.
#[inline]
pub fn hugetlb_free_2mib_count() -> usize {
    HUGETLB_POOL.free_count(HugeTlbSize::TwoMiB)
}

/// Returns number of available 1 GiB huge pages in pool.
#[inline]
pub fn hugetlb_free_1gib_count() -> usize {
    HUGETLB_POOL.free_count(HugeTlbSize::OneGiB)
}

/// Initializes hugetlb pool at boot.
///
/// # Safety
/// CPL 0.
pub unsafe fn init(n_2mib: usize, n_1gib: usize) {
    HUGETLB_POOL.init(n_2mib, n_1gib);
}
