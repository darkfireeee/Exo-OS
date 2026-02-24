// kernel/src/memory/physical/zone/normal.rs
//
// Zone NORMAL — mémoire physique >= 4 GiB (sur systèmes 64 bits).
// Zone principale sur x86_64 : toute la RAM au-dessus de 4 GiB.
// Les allocations noyau ordinaires proviennent de cette zone.
// Couche 0 — aucune dépendance externe.

use crate::memory::core::{PhysAddr, ZoneType, ZONE_DMA32_END};
use super::ZoneDescriptor;

/// Zone NORMAL — mémoire principale du système (>= 4 GiB sur 64 bits).
pub struct NormalZone {
    pub desc: ZoneDescriptor,
}

impl NormalZone {
    /// Crée la zone NORMAL.
    /// `mem_end_phys` = limite haute de la RAM dans cette zone.
    pub const fn new(numa_node: u8, mem_end_phys: PhysAddr, reserved_frames: usize) -> Self {
        let phys_start = PhysAddr::new(ZONE_DMA32_END as u64);
        let phys_end   = if mem_end_phys.as_u64() > phys_start.as_u64() {
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
            )
        }
    }

    /// Vérifie si une adresse appartient à la zone NORMAL.
    #[inline(always)]
    pub const fn is_normal_addr(addr: PhysAddr) -> bool {
        addr.as_usize() >= ZONE_DMA32_END
    }
}
