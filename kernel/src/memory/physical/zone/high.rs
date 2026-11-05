// kernel/src/memory/physical/zone/high.rs
//
// HIGH Zone — high memory on 32-bit systems (>896 MiB).
// On x86_64, this zone is empty (all frames are directly accessible).
// Preserved for architectural compatibility.
// Layer 0 — no external dependencies.

use super::ZoneDescriptor;
use crate::memory::core::{PhysAddr, ZoneType};

/// HIGH Zone — 32-bit high memory only (empty on x86_64).
pub struct HighZone {
    pub desc: ZoneDescriptor,
}

impl HighZone {
    /// Creates an empty HIGH zone (x86_64 — always empty).
    pub const fn new_empty(numa_node: u8) -> Self {
        HighZone {
            desc: ZoneDescriptor::new(
                ZoneType::High,
                numa_node,
                PhysAddr::new(0),
                PhysAddr::new(0),
                0,
                0,
            ),
        }
    }

    /// On x86_64, the HIGH zone is always empty.
    #[inline(always)]
    pub const fn is_empty_on_x86_64() -> bool {
        true
    }
}
