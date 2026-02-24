// kernel/src/memory/physical/zone/dma32.rs
//
// Zone DMA32 — mémoire physique [16 MiB, 4 GiB).
// Pour les devices PCIe avec DMA sur 32 bits d'adresse.
// Couche 0 — aucune dépendance externe.

use crate::memory::core::{PhysAddr, ZoneType, ZONE_DMA_END, ZONE_DMA32_END};
use super::ZoneDescriptor;

/// Zone DMA32 — adresses physiques [16 MiB, 4 GiB).
pub struct Dma32Zone {
    pub desc: ZoneDescriptor,
}

impl Dma32Zone {
    /// Crée la zone DMA32.
    /// `mem_end_phys` = fin effective de la RAM dans cette plage.
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
            )
        }
    }

    /// Vérifie si une adresse appartient à la zone DMA32.
    #[inline(always)]
    pub const fn is_dma32_addr(addr: PhysAddr) -> bool {
        addr.as_usize() >= ZONE_DMA_END && addr.as_usize() < ZONE_DMA32_END
    }

    pub const MAX_SIZE: usize = ZONE_DMA32_END - ZONE_DMA_END;
}
