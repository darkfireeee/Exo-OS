// kernel/src/memory/physical/zone/dma.rs
//
// Zone DMA — mémoire physique < 16 MiB.
// Réservée aux devices legacy 32 bits (ISA DMA, vieux contrôleurs PCI).
// Couche 0 — aucune dépendance externe.

use super::ZoneDescriptor;
use crate::memory::core::{PhysAddr, ZoneType, ZONE_DMA_END};

/// Zone DMA — adresses physiques [0, 16 MiB).
pub struct DmaZone {
    pub desc: ZoneDescriptor,
}

impl DmaZone {
    /// Crée la zone DMA pour le nœud NUMA `numa_node`.
    /// `mem_end` est la fin effective de la RAM DMA détectée par l'E820.
    /// Elle est plafonnée à ZONE_DMA_END (16 MiB).
    pub const fn new(numa_node: u8, mem_end_phys: PhysAddr, reserved_frames: usize) -> Self {
        // La zone DMA commence toujours à 0 (les premiers 16 MiB)
        // On réserve les 4 premiers KiB (page nulle) pour éviter NULL == valid address
        let phys_start = PhysAddr::new(4096); // Réserver la page 0
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

    /// Vérifie si une adresse physique peut appartenir à la zone DMA.
    #[inline(always)]
    pub const fn is_dma_addr(addr: PhysAddr) -> bool {
        addr.as_usize() < ZONE_DMA_END
    }

    /// Taille maximale de la zone en octets.
    pub const MAX_SIZE: usize = ZONE_DMA_END;
}
