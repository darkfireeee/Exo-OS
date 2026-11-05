// kernel/src/memory/physical/zone/dma32.rs
//
// DMA32 Zone — physical memory [16 MiB, 4 GiB).
// For PCIe devices with DMA using 32-bit addresses.
// Layer 0 — no external dependencies.

use super::ZoneDescriptor;
use crate::memory::core::{PhysAddr, ZoneType, ZONE_DMA32_END, ZONE_DMA_END};

/// DMA32 Zone — physical addresses [16 MiB, 4 GiB).
pub struct Dma32Zone {
    pub desc: ZoneDescriptor,
}

impl Dma32Zone {
    /// Creates the DMA32 zone.
    /// `mem_end_phys` = effective end of RAM within this range.
    pub const fn new(numa_node: u8, mem_end_phys: PhysAddr, reserved_frames: usize) -> Self {
        let phys_start = PhysAddr::new(ZONE_DMA_END as u64);
        let phys_end_raw = if mem_end_phys.as_u64() < ZONE_DMA32_END as u64 {
            mem_end_phys.as_u64()
        } else {
            ZONE_DMA32_END as u64
        };
        let phys_end = PhysAddr::new(phys_end_raw);
        let total = if phys_end.as_u64() > phys_start.as_u64() {
            ((phys_end.as_u64() - phys_start.as_u64()) / 4096) as usize
        } else {
            0
        };

        Dma32Zone {
            desc: ZoneDescriptor::new(
                ZoneType::Dma32,
                numa_node,
                phys_start,
                phys_end,
                total,
                reserved_frames,
            ),
        }
    }

    /// Checks whether an address belongs to the DMA32 zone.
    #[inline(always)]
    pub const fn is_dma32_addr(addr: PhysAddr) -> bool {
        addr.as_usize() >= ZONE_DMA_END && addr.as_usize() < ZONE_DMA32_END
    }

    pub const MAX_SIZE: usize = ZONE_DMA32_END - ZONE_DMA_END;
}
