// kernel/src/memory/physical/zone/dma.rs
//
// DMA Zone — physical memory < 16 MiB.
// Reserved for legacy 32-bit devices (ISA DMA, old PCI controllers).
// Layer 0 — no external dependencies.

use super::ZoneDescriptor;
use crate::memory::core::{PhysAddr, ZoneType, ZONE_DMA_END};

/// DMA Zone — physical addresses [0, 16 MiB).
pub struct DmaZone {
    pub desc: ZoneDescriptor,
}

impl DmaZone {
    /// Creates DMA zone for NUMA node `numa_node`.
    /// `mem_end` is the effective end of DMA RAM detected by E820.
    /// It is capped at ZONE_DMA_END (16 MiB).
    pub const fn new(numa_node: u8, mem_end_phys: PhysAddr, reserved_frames: usize) -> Self {
        // DMA zone always starts at 0 (first 16 MiB)
        // We reserve the first 4 KiB (null page) to avoid NULL == valid address
        let phys_start = PhysAddr::new(4096); // Reserve page 0
        let phys_end = PhysAddr::new(if mem_end_phys.as_u64() < ZONE_DMA_END as u64 {
            mem_end_phys.as_u64()
        } else {
            ZONE_DMA_END as u64
        });
        let total = if phys_end.as_u64() > phys_start.as_u64() {
            ((phys_end.as_u64() - phys_start.as_u64()) / 4096) as usize
        } else {
            0
        };

        DmaZone {
            desc: ZoneDescriptor::new(
                ZoneType::Dma,
                numa_node,
                phys_start,
                phys_end,
                total,
                reserved_frames,
            ),
        }
    }

    /// Checks whether a physical address can belong to the DMA zone.
    #[inline(always)]
    pub const fn is_dma_addr(addr: PhysAddr) -> bool {
        addr.as_usize() < ZONE_DMA_END
    }

    /// Maximum zone size in bytes.
    pub const MAX_SIZE: usize = ZONE_DMA_END;
}
