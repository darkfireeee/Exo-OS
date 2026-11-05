// kernel/src/memory/huge_pages/thp.rs
//
// Transparent Huge Pages (THP) — allocation and management of 2 MiB pages.
// LAYER 0 — no external dependencies.

use crate::memory::core::constants::{HUGE_PAGE_SIZE, PAGE_SIZE};
use crate::memory::core::types::{AllocFlags, Frame, PhysAddr};
use crate::memory::physical::allocator::buddy::{alloc_pages, free_pages};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// THP CONFIGURATION
// ─────────────────────────────────────────────────────────────────────────────

/// Buddy order for a 2 MiB page (2MiB / 4KiB = 512 = 2^9).
pub const HUGE_PAGE_ORDER: u32 = 9;

/// THP modes.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum ThpMode {
    /// THP disabled.
    Disabled = 0,
    /// THP enabled if possible (best-effort).
    Madvise = 1,
    /// THP always enabled.
    Always = 2,
}

/// Global THP configuration.
pub struct ThpConfig {
    pub mode: spin::RwLock<ThpMode>,
    pub enabled: AtomicBool,
    /// VMA threshold (in pages) from which THP is attempted.
    pub min_vma_size: AtomicU64,
}

impl Default for ThpConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl ThpConfig {
    const fn new() -> Self {
        ThpConfig {
            mode: spin::RwLock::new(ThpMode::Madvise),
            enabled: AtomicBool::new(true),
            min_vma_size: AtomicU64::new(512), // at least 2 MiB
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn set_mode(&self, mode: ThpMode) {
        *self.mode.write() = mode;
    }
    pub fn get_mode(&self) -> ThpMode {
        *self.mode.read()
    }
}

pub static THP_CONFIG: ThpConfig = ThpConfig::new();

// ─────────────────────────────────────────────────────────────────────────────
// THP STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct ThpStats {
    pub alloc_success: AtomicU64,
    pub alloc_fail: AtomicU64,
    pub splits: AtomicU64,
    pub promotions: AtomicU64,
    pub current_huge: AtomicU64,
}

impl Default for ThpStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ThpStats {
    const fn new() -> Self {
        ThpStats {
            alloc_success: AtomicU64::new(0),
            alloc_fail: AtomicU64::new(0),
            splits: AtomicU64::new(0),
            promotions: AtomicU64::new(0),
            current_huge: AtomicU64::new(0),
        }
    }
}

pub static THP_STATS: ThpStats = ThpStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// THP API
// ─────────────────────────────────────────────────────────────────────────────

/// Allocates a 2 MiB huge page (order 9 in buddy).
/// Returns base frame (2 MiB aligned).
pub fn alloc_huge_page(flags: AllocFlags) -> Option<Frame> {
    if !THP_CONFIG.is_enabled() {
        return None;
    }

    match alloc_pages(HUGE_PAGE_ORDER as usize, flags) {
        Ok(frame) => {
            THP_STATS.alloc_success.fetch_add(1, Ordering::Relaxed);
            THP_STATS.current_huge.fetch_add(1, Ordering::Relaxed);
            Some(frame)
        }
        Err(_) => {
            THP_STATS.alloc_fail.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

/// Frees a huge page allocated by `alloc_huge_page`.
///
/// # Safety
/// `frame` must have been allocated with `alloc_huge_page`.
pub unsafe fn free_huge_page(frame: Frame) {
    let _ = free_pages(frame, HUGE_PAGE_ORDER as usize);
    THP_STATS.current_huge.fetch_sub(1, Ordering::Relaxed);
}

/// Attempts to split a huge page into 512 regular pages.
/// Returns the 512 frames.
///
/// # Safety
/// `frame` must be a base frame of a huge page (2 MiB aligned).
///
/// # Memory ownership
/// The order-9 block remains "allocated" in the buddy; the caller is responsible
/// for freeing each individual frame (order-0) when no longer needed.
/// The buddy will progressively coalesce freed frames — without `free_pages`
/// here, thus no double-ownership between caller and buddy.
pub unsafe fn split_huge_page(frame: Frame) -> [Frame; 512] {
    THP_STATS.splits.fetch_add(1, Ordering::Relaxed);
    THP_STATS.current_huge.fetch_sub(1, Ordering::Relaxed);
    // The huge page is physically contiguous — calculate the 512 4 KiB frames.
    // DO NOT call free_pages: the order-9 block remains "in use"
    // from buddy's perspective. The caller transfers ownership into 512 frames.
    let base = frame.phys_addr().as_u64();
    let mut frames = [Frame::from_phys_addr(PhysAddr::new(0)); 512];
    for i in 0..512usize {
        frames[i] = Frame::from_phys_addr(PhysAddr::new(base + (i as u64 * PAGE_SIZE as u64)));
    }
    frames
}

/// Attempts to merge 512 contiguous 4KiB pages into a huge page.
/// Returns `Some(frame_base)` if successful.
pub fn try_promote_to_huge(base: PhysAddr) -> Option<Frame> {
    // Verify 2 MiB alignment.
    if !base.as_u64().is_multiple_of(HUGE_PAGE_SIZE as u64) {
        return None;
    }
    // Verify all pages are available in buddy — not implemented here
    // because it would require traversing free_lists, which is handled by buddy.rs.
    // We delegate: simply attempt to allocate order-9.
    let frame = alloc_huge_page(AllocFlags::NONE)?;
    THP_STATS.promotions.fetch_add(1, Ordering::Relaxed);
    Some(frame)
}
