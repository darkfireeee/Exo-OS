// kernel/src/memory/physical/zone/mod.rs
//
// Physical memory zones — Exo-OS Layer 0.
// Each zone is a RAM partition with its own constraints.

pub mod dma;
pub mod dma32;
pub mod high;
pub mod movable;
pub mod normal;

use crate::memory::core::{AllocFlags, Frame, PhysAddr, ZoneType, ZONE_DMA32_END, ZONE_DMA_END};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// ZONE DESCRIPTOR — common structure across all zones
// ─────────────────────────────────────────────────────────────────────────────

/// Memory zone descriptor — statistics and limits.
///
/// Shared across all zones, extended by their specific module.
#[repr(C, align(64))]
pub struct ZoneDescriptor {
    /// Type of this zone.
    pub zone_type: ZoneType,
    /// Owning NUMA node.
    pub numa_node: u8,
    /// Padding.
    _pad0: [u8; 6],
    /// Physical start address of the zone.
    pub phys_start: PhysAddr,
    /// Physical end address of the zone (exclusive).
    pub phys_end: PhysAddr,
    /// Total number of frames in this zone.
    pub total_frames: usize,
    /// Currently free frames count.
    free_frames: AtomicUsize,
    /// Reserved frames count (firmware/BIOS).
    pub reserved_frames: usize,
    /// Successful allocation count from this zone.
    alloc_success: AtomicU64,
    /// Failed allocation count in this zone.
    alloc_failures: AtomicU64,
    /// Free operations count performed in this zone.
    free_count: AtomicU64,
    /// Low watermark: below this threshold, kswapd wakes up.
    pub watermark_low: usize,
    /// Minimum watermark: below this threshold, only emergency allocations pass.
    pub watermark_min: usize,
    /// High watermark: above this threshold, kswapd stops.
    pub watermark_high: usize,
    /// Padding to reach 128 bytes.
    /// Calculation: 128 - 1(zone_type) - 1(numa_node) - 6(_pad0)
    ///             - 16(phys_start+phys_end) - 24(total+free+reserved frames)
    ///             - 24(alloc_success+alloc_failures+free_count)
    ///             - 24(watermark_low+min+high) = 128 - 96 = 32
    _pad1: [u8; 32],
}

const _: () = assert!(
    core::mem::size_of::<ZoneDescriptor>() == 128,
    "ZoneDescriptor doit faire exactement 128 bytes (2 cache lines)"
);

impl ZoneDescriptor {
    /// Creates a zone descriptor.
    pub const fn new(
        zone_type: ZoneType,
        numa_node: u8,
        phys_start: PhysAddr,
        phys_end: PhysAddr,
        total_frames: usize,
        reserved_frames: usize,
    ) -> Self {
        let wm_min = total_frames / 100; // 1%
        let wm_low = total_frames * 3 / 100; // 3%
        let wm_high = total_frames * 5 / 100; // 5%

        ZoneDescriptor {
            zone_type,
            numa_node,
            _pad0: [0u8; 6],
            phys_start,
            phys_end,
            total_frames,
            free_frames: AtomicUsize::new(total_frames - reserved_frames),
            reserved_frames,
            alloc_success: AtomicU64::new(0),
            alloc_failures: AtomicU64::new(0),
            free_count: AtomicU64::new(0),
            watermark_low: wm_low,
            watermark_min: wm_min,
            watermark_high: wm_high,
            _pad1: [0u8; 32],
        }
    }

    /// Returns the number of free frames.
    #[inline(always)]
    pub fn free_frames(&self) -> usize {
        self.free_frames.load(Ordering::Relaxed)
    }

    /// Decrements free frames counter (after allocation).
    #[inline(always)]
    pub fn dec_free(&self, count: usize) {
        self.free_frames.fetch_sub(count, Ordering::Relaxed);
        self.alloc_success.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments free frames counter (after release).
    #[inline(always)]
    pub fn inc_free(&self, count: usize) {
        self.free_frames.fetch_add(count, Ordering::Relaxed);
        self.free_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Records an allocation failure.
    #[inline(always)]
    pub fn record_failure(&self) {
        self.alloc_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Checks whether a frame belongs to this zone.
    #[inline(always)]
    pub fn contains(&self, frame: Frame) -> bool {
        let addr = frame.start_address();
        addr >= self.phys_start && addr < self.phys_end
    }

    /// Checks whether the zone is below the minimum watermark.
    #[inline(always)]
    pub fn is_below_watermark_min(&self) -> bool {
        self.free_frames() < self.watermark_min
    }

    /// Checks whether the zone is below the low watermark (kswapd must wake up).
    #[inline(always)]
    pub fn is_below_watermark_low(&self) -> bool {
        self.free_frames() < self.watermark_low
    }

    /// Statistics for this zone.
    pub fn stats(&self) -> ZoneStats {
        ZoneStats {
            zone_type: self.zone_type,
            numa_node: self.numa_node,
            total_frames: self.total_frames,
            free_frames: self.free_frames(),
            reserved_frames: self.reserved_frames,
            alloc_success: self.alloc_success.load(Ordering::Relaxed),
            alloc_failures: self.alloc_failures.load(Ordering::Relaxed),
            free_count: self.free_count.load(Ordering::Relaxed),
        }
    }
}

/// Memory zone statistics.
#[derive(Copy, Clone, Debug)]
pub struct ZoneStats {
    pub zone_type: ZoneType,
    pub numa_node: u8,
    pub total_frames: usize,
    pub free_frames: usize,
    pub reserved_frames: usize,
    pub alloc_success: u64,
    pub alloc_failures: u64,
    pub free_count: u64,
}

impl ZoneStats {
    /// Free memory percentage (0-100).
    pub fn free_percent(&self) -> u32 {
        if self.total_frames == 0 {
            return 0;
        }
        (self.free_frames * 100 / self.total_frames) as u32
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ZONE REGISTRY — global zone registry
// ─────────────────────────────────────────────────────────────────────────────

/// Selects the appropriate zone for the given allocation flags.
#[inline]
pub fn zone_for_flags(flags: AllocFlags) -> ZoneType {
    flags.required_zone()
}

/// Determines whether a physical address can satisfy allocation flags.
///
/// For example, a DMA allocation requires an address < 16 MiB.
#[inline]
pub fn addr_satisfies_flags(addr: PhysAddr, flags: AllocFlags) -> bool {
    let required = zone_for_flags(flags);
    match required {
        ZoneType::Dma => addr.as_usize() < ZONE_DMA_END,
        ZoneType::Dma32 => addr.as_usize() < ZONE_DMA32_END,
        ZoneType::Movable => true, // Any address can be movable
        _ => true,                 // Normal/High: no low address constraint
    }
}

pub use dma::DmaZone;
pub use dma32::Dma32Zone;
pub use high::HighZone;
pub use movable::MovableZone;
pub use normal::NormalZone;
