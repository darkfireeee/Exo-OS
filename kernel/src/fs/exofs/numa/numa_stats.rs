//! numa_stats.rs — Statistiques par nœud NUMA pour ExoFS (no_std).

use core::sync::atomic::{AtomicU64, Ordering};

pub const MAX_NUMA_NODES: usize = 8;

pub static NUMA_STATS: NumaStats = NumaStats::new_const();

/// Compteurs par nœud NUMA.
#[derive(Default)]
pub struct NumaNodeStats {
    pub allocs:     u64,
    pub frees:      u64,
    pub migrations: u64,
    pub bytes_alloc: u64,
}

pub struct NumaStats {
    allocs:      [AtomicU64; MAX_NUMA_NODES],
    frees:       [AtomicU64; MAX_NUMA_NODES],
    migrations:  [AtomicU64; MAX_NUMA_NODES],
    bytes_alloc: [AtomicU64; MAX_NUMA_NODES],
}

impl NumaStats {
    pub const fn new_const() -> Self {
        macro_rules! arr {
            () => { [
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
                AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
            ]};
        }
        Self {
            allocs:      arr!(),
            frees:       arr!(),
            migrations:  arr!(),
            bytes_alloc: arr!(),
        }
    }

    pub fn record_alloc(&self, node: usize, bytes: u64) {
        if node >= MAX_NUMA_NODES { return; }
        self.allocs[node].fetch_add(1, Ordering::Relaxed);
        self.bytes_alloc[node].fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_free(&self, node: usize, bytes: u64) {
        if node >= MAX_NUMA_NODES { return; }
        self.frees[node].fetch_add(1, Ordering::Relaxed);
        let cur = self.bytes_alloc[node].load(Ordering::Relaxed);
        self.bytes_alloc[node].store(cur.saturating_sub(bytes), Ordering::Relaxed);
    }

    pub fn record_migration(&self, from_node: usize) {
        if from_node >= MAX_NUMA_NODES { return; }
        self.migrations[from_node].fetch_add(1, Ordering::Relaxed);
    }

    pub fn node_stats(&self, node: usize) -> NumaNodeStats {
        if node >= MAX_NUMA_NODES { return NumaNodeStats::default(); }
        NumaNodeStats {
            allocs:      self.allocs[node].load(Ordering::Relaxed),
            frees:       self.frees[node].load(Ordering::Relaxed),
            migrations:  self.migrations[node].load(Ordering::Relaxed),
            bytes_alloc: self.bytes_alloc[node].load(Ordering::Relaxed),
        }
    }
}
