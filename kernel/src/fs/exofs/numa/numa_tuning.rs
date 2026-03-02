//! numa_tuning.rs — Politique NUMA adaptative pour ExoFS (no_std).

use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use super::numa_stats::NUMA_STATS;
use super::numa_placement::{PlacementStrategy, NUMA_PLACEMENT};

pub static NUMA_POLICY: NumaPolicy = NumaPolicy::new_const();

/// Seuils de déclenchement de la politique NUMA.
pub struct NumaPolicy {
    migrate_threshold_pct: AtomicU8,   // % de déséquilibre qui déclenche migration.
    auto_tune_enabled:     AtomicU8,   // 0=off, 1=on.
    tune_interval_ticks:   AtomicU64,  // Intervalle min entre réajustements.
    last_tune_tick:        AtomicU64,
}

impl NumaPolicy {
    pub const fn new_const() -> Self {
        Self {
            migrate_threshold_pct: AtomicU8::new(30),
            auto_tune_enabled:     AtomicU8::new(0),
            tune_interval_ticks:   AtomicU64::new(100_000),
            last_tune_tick:        AtomicU64::new(0),
        }
    }

    pub fn configure(&self, threshold_pct: u8, auto: bool, interval: u64) {
        self.migrate_threshold_pct.store(threshold_pct, Ordering::Relaxed);
        self.auto_tune_enabled.store(u8::from(auto), Ordering::Relaxed);
        self.tune_interval_ticks.store(interval, Ordering::Relaxed);
    }

    /// Évalue le déséquilibre entre nœuds et ajuste la stratégie si nécessaire.
    pub fn evaluate(&self, current_tick: u64) {
        if self.auto_tune_enabled.load(Ordering::Relaxed) == 0 { return; }
        let last = self.last_tune_tick.load(Ordering::Relaxed);
        let interval = self.tune_interval_ticks.load(Ordering::Relaxed);
        if current_tick.saturating_sub(last) < interval { return; }
        self.last_tune_tick.store(current_tick, Ordering::Relaxed);

        // Calcule le déséquilibre entre les 8 nœuds potentiels.
        let mut total: u64 = 0;
        let mut max_alloc: u64 = 0;
        let mut min_alloc: u64 = u64::MAX;
        for n in 0..8usize {
            let s = NUMA_STATS.node_stats(n);
            if s.allocs == 0 { continue; }
            total  = total.saturating_add(s.bytes_alloc);
            if s.bytes_alloc > max_alloc { max_alloc = s.bytes_alloc; }
            if s.bytes_alloc < min_alloc { min_alloc = s.bytes_alloc; }
        }
        if total == 0 || min_alloc == u64::MAX { return; }

        let imbalance_pct = if max_alloc > 0 {
            ((max_alloc - min_alloc) * 100 / max_alloc) as u8
        } else {
            0
        };

        let thresh = self.migrate_threshold_pct.load(Ordering::Relaxed);
        if imbalance_pct > thresh {
            NUMA_PLACEMENT.init(
                NUMA_PLACEMENT.n_nodes() as u8,
                PlacementStrategy::LeastUsed,
            );
        } else {
            NUMA_PLACEMENT.init(
                NUMA_PLACEMENT.n_nodes() as u8,
                PlacementStrategy::LocalFirst,
            );
        }
    }
}
