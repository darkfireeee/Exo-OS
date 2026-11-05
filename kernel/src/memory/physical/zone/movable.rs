// kernel/src/memory/physical/zone/movable.rs
//
// MOVABLE Zone — movable pages for huge pages defragmentation.
// MOVABLE pages can be migrated towards other NUMA nodes or
// consolidated to form 2MiB/1GiB huge pages.
// Layer 0 — no external dependencies.

use super::ZoneDescriptor;
use crate::memory::core::{PhysAddr, ZoneType};

/// MOVABLE Zone — allocatable migratable pages.
///
/// Pages allocated in this zone can be migrated by the
/// memory compactor. Ideal for userspace MOVABLE allocations.
pub struct MovableZone {
    pub desc: ZoneDescriptor,
}

impl MovableZone {
    /// Creates the MOVABLE zone.
    /// `phys_start`/`phys_end` are configured by NUMA policy at boot.
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

    /// Creates an empty MOVABLE zone (not configured).
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
