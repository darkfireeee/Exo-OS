// kernel/src/memory/physical/zone/high.rs
//
// Zone HIGH — mémoire haute sur systèmes 32 bits (>896 MiB).
// Sur x86_64, cette zone est vide (tous les frames sont accessibles directement).
// Conservée pour compatibilité architecturale.
// Couche 0 — aucune dépendance externe.

use super::ZoneDescriptor;
use crate::memory::core::{PhysAddr, ZoneType};

/// Zone HIGH — mémoire haute 32 bits uniquement (vide sur x86_64).
pub struct HighZone {
    pub desc: ZoneDescriptor,
}

impl HighZone {
    /// Crée une zone HIGH vide (x86_64 — toujours vide).
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

    /// Sur x86_64, la zone HIGH est toujours vide.
    #[inline(always)]
    pub const fn is_empty_on_x86_64() -> bool {
        true
    }
}
