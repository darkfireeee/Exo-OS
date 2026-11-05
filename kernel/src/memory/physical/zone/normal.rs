// kernel/src/memory/physical/zone/normal.rs
//
// NORMAL Zone — physical memory >= 4 GiB (on 64-bit systems).
// Main zone on x86_64: all RAM above 4 GiB.
// Ordinary kernel allocations come from this zone.
// Layer 0 — no external dependencies.

use super::ZoneDescriptor;
use crate::memory::core::{PhysAddr, ZoneType, ZONE_DMA32_END};

/// NORMAL Zone — main system memory (>= 4 GiB on 64-bit).
pub struct NormalZone {
    pub desc: ZoneDescriptor,
}

impl NormalZone {
    /// Creates the NORMAL zone.
    /// `mem_end_phys` = upper RAM limit in this zone.
    pub const fn new(numa_node: u8, mem_end_phys: PhysAddr, reserved_frames: usize) -> Self {
        let phys_start = PhysAddr::new(ZONE_DMA32_END as u64);
        let phys_end = if mem_end_phys.as_u64() > phys_start.as_u64() {
            mem_end_phys
        } else {
            phys_start
        };
        let total = if phys_end.as_u64() > phys_start.as_u64() {
            ((phys_end.as_u64() - phys_start.as_u64()) / 4096) as usize
        } else {
            0
        };
        NormalZone {
            desc: ZoneDescriptor::new(
                ZoneType::Normal,
                numa_node,
                phys_start,
                phys_end,
                total,
                reserved_frames,
            ),
        }
    }

    /// Checks whether an address belongs to the NORMAL zone.
    #[inline(always)]
    pub const fn is_normal_addr(addr: PhysAddr) -> bool {
        addr.as_usize() >= ZONE_DMA32_END
    }
}
