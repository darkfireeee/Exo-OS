// ipc/shared_memory/pool.rs — Pre-allocated SHM page pool for Exo-OS
//
// Static pool of SHM_POOL_PAGES (256) pages of 4 KiB each.
// O(1) allocations via occupancy bitmap (AtomicU64 x MAX_POOL_WORDS).
// Invariant: all pages have the NO_COW flag set.
//
// Performance constraint: alloc < 100 ns (lock-free CAS on bitmap)

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::ipc::core::constants::SHM_POOL_PAGES;
use crate::ipc::shared_memory::page::{PageFlags, PhysAddr, ShmPage, PAGE_SIZE};

// ---------------------------------------------------------------------------
// Allocation bitmap — 256 pages = 4 64-bit words
// ---------------------------------------------------------------------------

/// Number of u64 words in bitmap (SHM_POOL_PAGES / 64)
pub const POOL_BITMAP_WORDS: usize = SHM_POOL_PAGES / 64;

/// Base physical address of pool (fixed at startup by init_shm_pool())
static POOL_BASE_PHYS: AtomicU64 = AtomicU64::new(0);

/// Free slots bitmap (bit=1 -> free, bit=0 -> occupied)
static POOL_BITMAP: [AtomicU64; POOL_BITMAP_WORDS] = [
    AtomicU64::new(u64::MAX),
    AtomicU64::new(u64::MAX),
    AtomicU64::new(u64::MAX),
    AtomicU64::new(u64::MAX),
];

/// Page descriptors
static POOL_PAGES: [ShmPage; SHM_POOL_PAGES] = {
    const INIT: ShmPage = ShmPage::new_uninit();
    [INIT; SHM_POOL_PAGES]
};

/// Free page counter (approximate, for fast diagnostics)
static POOL_FREE_COUNT: AtomicUsize = AtomicUsize::new(SHM_POOL_PAGES);

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initializes the SHM pool with a base physical address.
/// To be called once at kernel startup (from ipc_init()).
///
/// # SAFETY
/// Must be called BEFORE any SHM allocation. `base_phys` must point
/// to a physical region of `SHM_POOL_PAGES * PAGE_SIZE` bytes reserved
/// by the memory manager.
pub unsafe fn init_shm_pool(base_phys: u64) {
    POOL_BASE_PHYS.store(base_phys, Ordering::Relaxed);

    for i in 0..SHM_POOL_PAGES {
        let phys = PhysAddr(base_phys + (i * PAGE_SIZE) as u64);
        POOL_PAGES[i].init(phys, i as u32, PageFlags::SHM_DEFAULT);
    }

    // Mark all pages as free
    for w in POOL_BITMAP.iter() {
        w.store(u64::MAX, Ordering::Release);
    }
    POOL_FREE_COUNT.store(SHM_POOL_PAGES, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Page allocation / deallocation
// ---------------------------------------------------------------------------

/// Allocates an SHM page from the global pool.
///
/// Algorithm:
///   1. Traverse the 4 bitmap words
///   2. For each non-zero word, isolate lowest bit (trailing_zeros)
///   3. Atomic CAS to reserve bit (1 -> 0)
///   4. Return page index = word * 64 + bit
///
/// Complexity: O(POOL_BITMAP_WORDS) = O(1)
/// Lock-free guarantee: no mutex, only one CAS per allocation.
pub fn shm_page_alloc() -> Option<usize> {
    for (word_idx, word) in POOL_BITMAP.iter().enumerate() {
        loop {
            let v = word.load(Ordering::Acquire);
            if v == 0 {
                break; // this word is full, move to next
            }
            let bit = v.trailing_zeros() as usize;
            let mask = 1u64 << bit;
            // CAS attempt: mark bit as occupied (1 -> 0)
            match word.compare_exchange_weak(v, v & !mask, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    let page_idx = word_idx * 64 + bit;
                    if page_idx < SHM_POOL_PAGES {
                        POOL_PAGES[page_idx].ref_acquire();
                        POOL_FREE_COUNT.fetch_sub(1, Ordering::Relaxed);
                        return Some(page_idx);
                    }
                    // Out of bounds index (should not happen) — release
                    word.fetch_or(mask, Ordering::Release);
                    return None;
                }
                Err(_) => {
                    // Contention: back to start of loop
                    core::hint::spin_loop();
                    continue;
                }
            }
        }
    }
    None // pool exhausted
}

/// Releases page at index `page_idx` in global pool.
/// Returns `true` if deallocation succeeded.
pub fn shm_page_free(page_idx: usize) -> bool {
    if page_idx >= SHM_POOL_PAGES {
        return false;
    }

    let freed = POOL_PAGES[page_idx].ref_release();
    if freed {
        // Reset bit to 1 in bitmap
        let word_idx = page_idx / 64;
        let bit = page_idx % 64;
        let mask = 1u64 << bit;
        POOL_BITMAP[word_idx].fetch_or(mask, Ordering::Release);
        POOL_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    freed
}

/// Returns reference to ShmPage for `page_idx`.
pub fn shm_page_ref(page_idx: usize) -> Option<&'static ShmPage> {
    if page_idx < SHM_POOL_PAGES {
        Some(&POOL_PAGES[page_idx])
    } else {
        None
    }
}

/// Returns physical address of page `page_idx`.
pub fn shm_page_phys(page_idx: usize) -> Option<PhysAddr> {
    if page_idx < SHM_POOL_PAGES {
        Some(POOL_PAGES[page_idx].phys_addr)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Pool statistics
// ---------------------------------------------------------------------------

/// Returns a snapshot of pool statistics.
pub fn shm_pool_stats() -> ShmPoolStats {
    let free = POOL_FREE_COUNT.load(Ordering::Relaxed);
    let mut total_reuses: u64 = 0;
    for p in POOL_PAGES.iter() {
        total_reuses += p.reuse_count.load(Ordering::Relaxed) as u64;
    }

    ShmPoolStats {
        total_pages: SHM_POOL_PAGES,
        free_pages: free,
        used_pages: SHM_POOL_PAGES.saturating_sub(free),
        base_phys: POOL_BASE_PHYS.load(Ordering::Relaxed),
        total_reuses,
    }
}

/// Snapshot of SHM pool statistics
#[derive(Debug, Clone, Copy)]
pub struct ShmPoolStats {
    pub total_pages: usize,
    pub free_pages: usize,
    pub used_pages: usize,
    pub base_phys: u64,
    pub total_reuses: u64,
}

// ---------------------------------------------------------------------------
// Contiguous ranges allocation (for large buffers)
// ---------------------------------------------------------------------------

/// Allocates `count` contiguous pages in the pool.
/// Returns index of first page, or `None` if impossible.
///
/// Note: linear O(SHM_POOL_PAGES) — reserved for rare/init allocations.
pub fn shm_alloc_contiguous(count: usize) -> Option<usize> {
    if count == 0 || count > SHM_POOL_PAGES {
        return None;
    }

    // Search for a range of `count` consecutive bits set to 1 in bitmap
    'outer: for start in 0..=(SHM_POOL_PAGES - count) {
        // Verify that all pages [start, start+count) are free
        for i in 0..count {
            let idx = start + i;
            let word_idx = idx / 64;
            let bit = idx % 64;
            let v = POOL_BITMAP[word_idx].load(Ordering::Acquire);
            if (v & (1u64 << bit)) == 0 {
                continue 'outer;
            }
        }

        // All free — try to reserve them atomically
        // (individual CAS are used — acceptable for a rare operation)
        let mut reserved = 0usize;
        for i in 0..count {
            let idx = start + i;
            let word_idx = idx / 64;
            let bit = idx % 64;
            let mask = 1u64 << bit;
            let v = POOL_BITMAP[word_idx].load(Ordering::Acquire);
            if (v & mask) == 0 {
                // Bit already taken by concurrent allocation — rollback
                for j in 0..reserved {
                    shm_page_free(start + j);
                }
                continue 'outer;
            }

            match POOL_BITMAP[word_idx].compare_exchange(
                v,
                v & !mask,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    POOL_PAGES[idx].ref_acquire();
                    POOL_FREE_COUNT.fetch_sub(1, Ordering::Relaxed);
                    reserved += 1;
                }
                Err(_) => {
                    // Contention — cancel previous reservations
                    for j in 0..reserved {
                        shm_page_free(start + j);
                    }
                    continue 'outer;
                }
            }
        }
        return Some(start);
    }
    None
}

/// Releases a range of `count` contiguous pages starting at `start`.
pub fn shm_free_contiguous(start: usize, count: usize) -> bool {
    if start + count > SHM_POOL_PAGES {
        return false;
    }
    for i in 0..count {
        shm_page_free(start + i);
    }
    true
}
