//! # memory/physical/allocator/buddy/mod.rs — Main Buddy physical allocator O(log n)
//!
//! Classical implementation of buddy system with per-order free-lists.
//! Layer 0 — no external dependencies.
//!
//! Principle:
//!   RAM is divided into blocks of size 2^order × PAGE_SIZE.
//!   Each order has a doubly-linked free-list.
//!   Allocation : traverse orders bottom-up to first available block,
//!                then split higher-order blocks.
//!   Free       : locate buddy and perform recursive coalescing.
//!
//! Invariants:
//!   - An order-k block has an address aligned to 2^k pages.
//!   - The buddy of block with address P is P ⊕ (1 << k).
//!   - Never any external fragmentation beyond 1 block of each order.

pub mod node;
pub mod stats;
pub mod zone;

pub use stats::BuddyZoneStats;
pub use zone::{reserve_frame_descriptor, unreserve_frame_descriptor, BuddyZone};

use core::sync::atomic::{AtomicBool, Ordering};
use crate::memory::core::{AllocError, AllocFlags, Frame, PhysAddr, ZoneType, PAGE_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL BUDDY ALLOCATOR — covers all zones
// ─────────────────────────────────────────────────────────────────────────────

/// Global buddy allocator covering all zones and NUMA nodes.
///
/// Allocation routing according to flags:
///   - AllocFlags::DMA    → BuddyZone DMA
///   - AllocFlags::DMA32  → BuddyZone DMA32
///   - Default           → BuddyZone Normal (NUMA-preferred)
pub struct GlobalBuddyAllocator {
    // One zone per ZoneType per NUMA node
    // Simplified: array [ZoneType::COUNT] (NUMA node 0 by default for now)
    zones: [BuddyZone; 4], // DMA, DMA32, Normal, Movable
    initialized: AtomicBool,
}

// SAFETY: GlobalBuddyAllocator is thread-safe via its internal spinlocks.
unsafe impl Sync for GlobalBuddyAllocator {}
unsafe impl Send for GlobalBuddyAllocator {}

impl Default for GlobalBuddyAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalBuddyAllocator {
    pub const fn new() -> Self {
        GlobalBuddyAllocator {
            zones: [const { BuddyZone::new_uninit() }; 4],
            initialized: AtomicBool::new(false),
        }
    }

    /// Allocates `2^order` contiguous pages according to flags.
    /// Automatically selects appropriate zone.
    #[inline]
    pub fn alloc_pages(&self, order: usize, flags: AllocFlags) -> Result<Frame, AllocError> {
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "GlobalBuddyAllocator non initialisé"
        );

        let zone_idx = self.zone_index_for(flags);
        let result = self.zones[zone_idx].alloc_pages(order, flags);

        // Fallback to higher zone if target zone is exhausted
        if result.is_err() && !flags.contains(AllocFlags::DMA) {
            for fallback_idx in (zone_idx + 1)..4 {
                let r = self.zones[fallback_idx].alloc_pages(order, flags);
                if r.is_ok() {
                    return r;
                }
            }
        }

        // Fallback to lower zone (Normal -> DMA32 on systems with < 4 GiB RAM).
        if result.is_err() && zone_idx > 1 {
            for fallback_idx in (1..zone_idx).rev() {
                let r = self.zones[fallback_idx].alloc_pages(order, flags);
                if r.is_ok() {
                    return r;
                }
            }
        }

        result
    }

    /// Frees a block of `2^order` pages.
    #[inline]
    pub fn free_pages(&self, frame: Frame, order: usize) -> Result<(), AllocError> {
        let phys = frame.start_address();
        // Determine enclosing zone
        for zone in &self.zones {
            if zone.is_initialized() && zone.phys_start <= phys && phys < zone.phys_end {
                return zone.free_pages(frame, order);
            }
        }
        Err(AllocError::ZoneUnavailable)
    }

    /// Allocates from requested NUMA node.
    pub fn alloc_on_node(
        &self,
        order: usize,
        flags: AllocFlags,
        numa_node: u8,
    ) -> Result<Frame, AllocError> {
        debug_assert!(
            self.initialized.load(Ordering::Acquire),
            "GlobalBuddyAllocator non initialisé"
        );

        // Phase 1: attempt in NUMA-local zones.
        for zone in &self.zones {
            if !zone.is_initialized() {
                continue;
            }
            if zone.numa_node != numa_node {
                continue;
            }
            if let Ok(frame) = zone.alloc_pages(order, flags) {
                return Ok(frame);
            }
        }

        // Phase 2: global fallback (cross-NUMA allocation).
        self.alloc_pages(order, flags)
    }

    #[inline]
    fn zone_index_for(&self, flags: AllocFlags) -> usize {
        match flags.required_zone() {
            ZoneType::Dma => 0,
            ZoneType::Dma32 => 1,
            ZoneType::Movable => 3,
            _ => 2, // Normal by default
        }
    }

    /// Returns total number of free frames across all zones.
    pub fn total_free_frames(&self) -> usize {
        self.zones
            .iter()
            .filter(|z| z.is_initialized())
            .map(|z| z.free_frames())
            .sum()
        }

    /// Returns total number of managed physical frames (free + used).
    pub fn total_frames(&self) -> usize {
        self.zones
            .iter()
            .filter(|z| z.is_initialized())
            .map(|z| z.total_frames_count())
            .sum()
    }

    /// Marks allocator as initialized.
    pub fn mark_initialized(&self) {
        self.initialized.store(true, Ordering::Release);
    }

    /// Initializes an allocator zone (called at boot, single-CPU).
    ///
    /// # Safety
    /// - Must be called EXACTLY ONCE per zone, before SMP.
    /// - `bitmap_buf` must be valid for `bitmap_words` × 8 bytes.
    pub unsafe fn init_zone(
        &self,
        zone_type: ZoneType,
        phys_start: PhysAddr,
        phys_end: PhysAddr,
        bitmap_buf: *mut u64,
        bitmap_words: usize,
    ) {
        let idx = (zone_type.index()).min(3);
        self.zones[idx].init(
            zone_type,
            0, /* NUMA node 0 */
            phys_start,
            phys_end,
            bitmap_buf,
            bitmap_words,
        );
    }

    /// Adds a free frame range to initialized zones covering this range.
    ///
    /// # Safety
    /// - Concerned zones must be initialized.
    /// - Range must not contain active kernel memory.
    pub unsafe fn add_free_zone_region(&self, start: PhysAddr, end: PhysAddr) {
        for zone in &self.zones {
            if !zone.is_initialized() {
                continue;
            }
            let zs = zone.phys_start.as_u64();
            let ze = zone.phys_end.as_u64();
            let rs = start.as_u64();
            let re = end.as_u64();
            if rs >= ze || re <= zs {
                continue;
            } // no overlap
            let cs = rs.max(zs);
            let ce = re.min(ze);
            if cs >= ce {
                continue;
            }
            let first_pfn = ((cs - zs) / PAGE_SIZE as u64) as usize;
            let last_pfn = ((ce - zs) / PAGE_SIZE as u64) as usize;
            zone.add_free_range(first_pfn, last_pfn);
        }
    }

    /// Returns reference to zone for given type.
    pub fn zone(&self, zone_type: ZoneType) -> Option<&BuddyZone> {
        let idx = match zone_type {
            ZoneType::Dma => 0,
            ZoneType::Dma32 => 1,
            ZoneType::Normal => 2,
            ZoneType::Movable => 3,
            ZoneType::High => return None,
        };
        let z = &self.zones[idx];
        if z.is_initialized() {
            Some(z)
        } else {
            None
        }
    }

    /// Mutable access to a zone (initialization only).
    ///
    /// # Safety
    /// Must be called only during init, in single-CPU.
    pub unsafe fn zone_mut(&self, zone_type: ZoneType) -> Option<&BuddyZone> {
        self.zone(zone_type)
    }
}

/// Global buddy allocator.
pub static BUDDY: GlobalBuddyAllocator = GlobalBuddyAllocator::new();

/// Allocates `2^order` contiguous physical pages.
#[inline(always)]
pub fn alloc_pages(order: usize, flags: AllocFlags) -> Result<Frame, AllocError> {
    BUDDY.alloc_pages(order, flags)
}

/// Frees a block of `2^order` pages.
#[inline(always)]
pub fn free_pages(frame: Frame, order: usize) -> Result<(), AllocError> {
    BUDDY.free_pages(frame, order)
}

/// Allocates exactly 1 physical page (order 0).
#[inline(always)]
pub fn alloc_page(flags: AllocFlags) -> Result<Frame, AllocError> {
    alloc_pages(0, flags)
}

/// Frees exactly 1 physical page.
#[inline(always)]
pub fn free_page(frame: Frame) -> Result<(), AllocError> {
    free_pages(frame, 0)
}

/// Reserves a physical frame against any buddy freeing/reallocation.
#[inline(always)]
pub fn reserve_frame(frame: Frame) {
    reserve_frame_descriptor(frame);
}

/// Reserves a series of contiguous physical frames.
#[inline(always)]
pub fn reserve_frames(frames: &[Frame]) {
    for &frame in frames {
        reserve_frame_descriptor(frame);
    }
}

/// Unpins a previously reserved physical frame.
#[inline(always)]
pub fn unreserve_frame(frame: Frame) {
    unreserve_frame_descriptor(frame);
}

/// Unpins a series of physical frames.
#[inline(always)]
pub fn unreserve_frames(frames: &[Frame]) {
    for &frame in frames {
        unreserve_frame_descriptor(frame);
    }
}
