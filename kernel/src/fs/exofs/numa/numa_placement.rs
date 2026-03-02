//! numa_placement.rs — Stratégie de placement NUMA pour les blobs ExoFS (no_std).

use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use crate::fs::exofs::core::BlobId;

pub static NUMA_PLACEMENT: NumaPlacement = NumaPlacement::new_const();

/// Stratégie de placement NUMA.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementStrategy {
    RoundRobin  = 0,   // Distribution cyclique entre nœuds.
    LocalFirst  = 1,   // Préférence au nœud local de la tâche courante.
    LeastUsed   = 2,   // Nœud avec le moins de blocs alloués.
}

pub struct NumaPlacement {
    strategy:    AtomicU8,
    n_nodes:     AtomicU8,
    rr_counter:  AtomicU64,
}

impl NumaPlacement {
    pub const fn new_const() -> Self {
        Self {
            strategy:   AtomicU8::new(PlacementStrategy::RoundRobin as u8),
            n_nodes:    AtomicU8::new(1),
            rr_counter: AtomicU64::new(0),
        }
    }

    pub fn init(&self, n_nodes: u8, strategy: PlacementStrategy) {
        self.n_nodes.store(n_nodes.max(1).min(8), Ordering::Relaxed);
        self.strategy.store(strategy as u8, Ordering::Relaxed);
    }

    /// Retourne le nœud NUMA préféré pour ce blob.
    pub fn n_nodes(&self) -> usize {
        self.n_nodes.load(Ordering::Relaxed) as usize
    }

    /// Retourne le nœud NUMA préféré pour ce blob.
    pub fn preferred_node(&self, blob_id: Option<BlobId>) -> usize {
        let n = self.n_nodes.load(Ordering::Relaxed) as usize;
        if n <= 1 { return 0; }

        let strat = self.strategy.load(Ordering::Relaxed);
        match strat {
            0 /* RoundRobin */ => {
                let c = self.rr_counter.fetch_add(1, Ordering::Relaxed);
                (c as usize) % n
            }
            1 /* LocalFirst */ => {
                // Hash de l'ID blob pour distribuer en locality-aware.
                if let Some(id) = blob_id {
                    let b = id.as_bytes();
                    let h = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
                    (h as usize) % n
                } else {
                    0
                }
            }
            _ /* LeastUsed */ => {
                // Sans accès direct aux stats ici, on utilise RoundRobin.
                let c = self.rr_counter.fetch_add(1, Ordering::Relaxed);
                (c as usize) % n
            }
        }
    }
}
