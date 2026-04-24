// kernel/src/memory/physical/zone/movable.rs
//
// Zone MOVABLE — pages déplaçables pour la défragmentation des huge pages.
// Les pages MOVABLE peuvent être migrées vers d'autres nœuds NUMA ou
// consolidées pour former des 2MiB/1GiB huge pages.
// Couche 0 — aucune dépendance externe.

use super::ZoneDescriptor;
use crate::memory::core::{PhysAddr, ZoneType};

/// Zone MOVABLE — allocation de pages migratables.
///
/// Les pages allouées dans cette zone peuvent être migrées par le
/// compacteur de mémoire. Idéal pour les allocations MOVABLE du userspace.
pub struct MovableZone {
    pub desc: ZoneDescriptor,
}

impl MovableZone {
    /// Crée la zone MOVABLE.
    /// `phys_start`/`phys_end` sont configurés par le policy NUMA au boot.
    pub const fn new(
        numa_node: u8,
        phys_start: PhysAddr,
        phys_end: PhysAddr,
        reserved_frames: usize,
    ) -> Self {
        let total = if phys_end.as_u64() > phys_start.as_u64() {
            ((phys_end.as_u64() - phys_start.as_u64()) / 4096) as usize
        } else {
            0
        };
        MovableZone {
            desc: ZoneDescriptor::new(
                ZoneType::Movable,
                numa_node,
                phys_start,
                phys_end,
                total,
                reserved_frames,
            ),
        }
    }

    /// Crée une zone MOVABLE vide (pas configurée).
    pub const fn new_empty(numa_node: u8) -> Self {
        MovableZone {
            desc: ZoneDescriptor::new(
                ZoneType::Movable,
                numa_node,
                PhysAddr::new(0),
                PhysAddr::new(0),
                0,
                0,
            ),
        }
    }
}
