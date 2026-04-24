// SPDX-License-Identifier: MIT
// ExoFS NUMA — Statistiques par nœud NUMA
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use super::numa_affinity::MAX_NUMA_NODES;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── NumaNodeStats (snapshot) ─────────────────────────────────────────────────

/// Snapshot read-only des compteurs d'un nœud.
#[derive(Clone, Copy, Debug, Default)]
pub struct NumaNodeStats {
    pub allocs: u64,
    pub frees: u64,
    pub bytes_alloc: u64,
    pub bytes_freed: u64,
    pub migrations_out: u64,
    pub migrations_in: u64,
    pub read_ops: u64,
    pub write_ops: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub errors: u64,
    pub pressure_events: u64,
}

impl NumaNodeStats {
    /// Allocations nettes (allocs − frees), ARITH-02.
    pub fn net_allocs(&self) -> u64 {
        self.allocs.saturating_sub(self.frees)
    }
    /// Octets nets alloués, ARITH-02.
    pub fn net_bytes(&self) -> u64 {
        self.bytes_alloc.saturating_sub(self.bytes_freed)
    }
    /// Ratio d'utilisation en ‰ par rapport à `capacity_bytes` (ARITH-02).
    pub fn usage_ppt(&self, capacity_bytes: u64) -> u64 {
        if capacity_bytes == 0 {
            return 0;
        }
        self.net_bytes()
            .saturating_mul(1000)
            .checked_div(capacity_bytes)
            .unwrap_or(1000)
    }
    /// Bandwidth write en octets/tick, ARITH-02.
    pub fn write_bw(&self, ticks_elapsed: u64) -> u64 {
        if ticks_elapsed == 0 {
            return 0;
        }
        self.write_bytes.checked_div(ticks_elapsed).unwrap_or(0)
    }
    /// Bandwidth read en octets/tick, ARITH-02.
    pub fn read_bw(&self, ticks_elapsed: u64) -> u64 {
        if ticks_elapsed == 0 {
            return 0;
        }
        self.read_bytes.checked_div(ticks_elapsed).unwrap_or(0)
    }
    /// Vrai si le nœud est sous pression.
    pub fn is_under_pressure(&self) -> bool {
        self.pressure_events > 0
    }
    /// Score de charge brut (somme pondérée, ARITH-02).
    pub fn load_score(&self) -> u64 {
        self.net_bytes()
            .saturating_add(self.migrations_out.saturating_mul(4096))
            .saturating_add(self.pressure_events.saturating_mul(8192))
    }
}

// ─── NumaStats ────────────────────────────────────────────────────────────────

/// Compteurs atomiques par nœud NUMA.
pub struct NumaStats {
    allocs: [AtomicU64; MAX_NUMA_NODES],
    frees: [AtomicU64; MAX_NUMA_NODES],
    bytes_alloc: [AtomicU64; MAX_NUMA_NODES],
    bytes_freed: [AtomicU64; MAX_NUMA_NODES],
    migrations_out: [AtomicU64; MAX_NUMA_NODES],
    migrations_in: [AtomicU64; MAX_NUMA_NODES],
    read_ops: [AtomicU64; MAX_NUMA_NODES],
    write_ops: [AtomicU64; MAX_NUMA_NODES],
    read_bytes: [AtomicU64; MAX_NUMA_NODES],
    write_bytes: [AtomicU64; MAX_NUMA_NODES],
    errors: [AtomicU64; MAX_NUMA_NODES],
    pressure_events: [AtomicU64; MAX_NUMA_NODES],
}

macro_rules! atomic8 {
    () => {
        [
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
            AtomicU64::new(0),
        ]
    };
}

impl NumaStats {
    pub const fn new_const() -> Self {
        Self {
            allocs: atomic8!(),
            frees: atomic8!(),
            bytes_alloc: atomic8!(),
            bytes_freed: atomic8!(),
            migrations_out: atomic8!(),
            migrations_in: atomic8!(),
            read_ops: atomic8!(),
            write_ops: atomic8!(),
            read_bytes: atomic8!(),
            write_bytes: atomic8!(),
            errors: atomic8!(),
            pressure_events: atomic8!(),
        }
    }

    #[inline]
    fn guard(&self, node: usize) -> bool {
        node < MAX_NUMA_NODES
    }

    // ── Enregistrements ───────────────────────────────────────────────────────

    /// Enregistre une allocation de `bytes` sur le nœud.
    pub fn record_alloc(&self, node: usize, bytes: u64) {
        if !self.guard(node) {
            return;
        }
        self.allocs[node].fetch_add(1, Ordering::Relaxed);
        self.bytes_alloc[node].fetch_add(bytes, Ordering::Relaxed);
    }

    /// Enregistre une libération de `bytes` sur le nœud.
    pub fn record_free(&self, node: usize, bytes: u64) {
        if !self.guard(node) {
            return;
        }
        self.frees[node].fetch_add(1, Ordering::Relaxed);
        self.bytes_freed[node].fetch_add(bytes, Ordering::Relaxed);
    }

    /// Enregistre une migration (arrivée sur `to`, départ de `from`).
    pub fn record_migration(&self, from: usize, to: usize) {
        if self.guard(from) {
            self.migrations_out[from].fetch_add(1, Ordering::Relaxed);
        }
        if self.guard(to) {
            self.migrations_in[to].fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Enregistre une lecture.
    pub fn record_read(&self, node: usize, bytes: u64) {
        if !self.guard(node) {
            return;
        }
        self.read_ops[node].fetch_add(1, Ordering::Relaxed);
        self.read_bytes[node].fetch_add(bytes, Ordering::Relaxed);
    }

    /// Enregistre une écriture.
    pub fn record_write(&self, node: usize, bytes: u64) {
        if !self.guard(node) {
            return;
        }
        self.write_ops[node].fetch_add(1, Ordering::Relaxed);
        self.write_bytes[node].fetch_add(bytes, Ordering::Relaxed);
    }

    /// Enregistre une erreur.
    pub fn record_error(&self, node: usize) {
        if !self.guard(node) {
            return;
        }
        self.errors[node].fetch_add(1, Ordering::Relaxed);
    }

    /// Enregistre un événement de pression mémoire.
    pub fn record_pressure(&self, node: usize) {
        if !self.guard(node) {
            return;
        }
        self.pressure_events[node].fetch_add(1, Ordering::Relaxed);
    }

    // ── Lecture ───────────────────────────────────────────────────────────────

    /// Snapshot des compteurs d'un nœud.
    pub fn node_stats(&self, node: usize) -> NumaNodeStats {
        if !self.guard(node) {
            return NumaNodeStats::default();
        }
        NumaNodeStats {
            allocs: self.allocs[node].load(Ordering::Relaxed),
            frees: self.frees[node].load(Ordering::Relaxed),
            bytes_alloc: self.bytes_alloc[node].load(Ordering::Relaxed),
            bytes_freed: self.bytes_freed[node].load(Ordering::Relaxed),
            migrations_out: self.migrations_out[node].load(Ordering::Relaxed),
            migrations_in: self.migrations_in[node].load(Ordering::Relaxed),
            read_ops: self.read_ops[node].load(Ordering::Relaxed),
            write_ops: self.write_ops[node].load(Ordering::Relaxed),
            read_bytes: self.read_bytes[node].load(Ordering::Relaxed),
            write_bytes: self.write_bytes[node].load(Ordering::Relaxed),
            errors: self.errors[node].load(Ordering::Relaxed),
            pressure_events: self.pressure_events[node].load(Ordering::Relaxed),
        }
    }

    /// Snapshot de tous les nœuds (RECUR-01 : while, OOM-02).
    pub fn all_nodes(&self) -> ExofsResult<Vec<(usize, NumaNodeStats)>> {
        let mut v = Vec::new();
        v.try_reserve(MAX_NUMA_NODES)
            .map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            v.push((i, self.node_stats(i)));
            i = i.wrapping_add(1);
        }
        Ok(v)
    }

    /// Nœud avec le plus faible score de charge (RECUR-01, ARITH-02).
    pub fn least_loaded_node(&self) -> usize {
        let mut best_node = 0usize;
        let mut best_score = u64::MAX;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            let s = self.node_stats(i).load_score();
            if s < best_score {
                best_score = s;
                best_node = i;
            }
            i = i.wrapping_add(1);
        }
        best_node
    }

    /// Nœud avec le plus grand score de charge (RECUR-01).
    pub fn most_loaded_node(&self) -> usize {
        let mut best_node = 0usize;
        let mut best_score = 0u64;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            let s = self.node_stats(i).load_score();
            if s > best_score {
                best_score = s;
                best_node = i;
            }
            i = i.wrapping_add(1);
        }
        best_node
    }

    /// Déséquilibre de charge entre nœuds en ‰ (ARITH-02, RECUR-01).
    pub fn imbalance_ppt(&self) -> u64 {
        let mut max_score = 0u64;
        let mut min_score = u64::MAX;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            let s = self.node_stats(i).load_score();
            if s > max_score {
                max_score = s;
            }
            if s < min_score {
                min_score = s;
            }
            i = i.wrapping_add(1);
        }
        if max_score == 0 || min_score == u64::MAX {
            return 0;
        }
        if min_score == u64::MAX {
            min_score = 0;
        }
        max_score
            .saturating_sub(min_score)
            .saturating_mul(1000)
            .checked_div(max_score)
            .unwrap_or(0)
    }

    /// Total allocations tous nœuds confondus (ARITH-02, RECUR-01).
    pub fn total_allocs(&self) -> u64 {
        let mut t = 0u64;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            t = t.saturating_add(self.allocs[i].load(Ordering::Relaxed));
            i = i.wrapping_add(1);
        }
        t
    }

    /// Total bytes alloués tous nœuds confondus (ARITH-02, RECUR-01).
    pub fn total_bytes_alloc(&self) -> u64 {
        let mut t = 0u64;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            t = t.saturating_add(self.bytes_alloc[i].load(Ordering::Relaxed));
            i = i.wrapping_add(1);
        }
        t
    }

    /// Total migrations tous nœuds confondus (ARITH-02, RECUR-01).
    pub fn total_migrations(&self) -> u64 {
        let mut t = 0u64;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            t = t.saturating_add(self.migrations_out[i].load(Ordering::Relaxed));
            i = i.wrapping_add(1);
        }
        t
    }

    /// Total erreurs tous nœuds confondus (ARITH-02).
    pub fn total_errors(&self) -> u64 {
        let mut t = 0u64;
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            t = t.saturating_add(self.errors[i].load(Ordering::Relaxed));
            i = i.wrapping_add(1);
        }
        t
    }

    /// Vrai si le système NUMA est sain (aucune erreur, déséquilibre < 800‰).
    pub fn is_healthy(&self) -> bool {
        self.total_errors() == 0 && self.imbalance_ppt() < 800
    }

    /// Réinitialise les compteurs d'un nœud.
    pub fn reset_node(&self, node: usize) {
        if !self.guard(node) {
            return;
        }
        self.allocs[node].store(0, Ordering::Relaxed);
        self.frees[node].store(0, Ordering::Relaxed);
        self.bytes_alloc[node].store(0, Ordering::Relaxed);
        self.bytes_freed[node].store(0, Ordering::Relaxed);
        self.migrations_out[node].store(0, Ordering::Relaxed);
        self.migrations_in[node].store(0, Ordering::Relaxed);
        self.read_ops[node].store(0, Ordering::Relaxed);
        self.write_ops[node].store(0, Ordering::Relaxed);
        self.read_bytes[node].store(0, Ordering::Relaxed);
        self.write_bytes[node].store(0, Ordering::Relaxed);
        self.errors[node].store(0, Ordering::Relaxed);
        self.pressure_events[node].store(0, Ordering::Relaxed);
    }

    /// Réinitialise tous les nœuds (RECUR-01).
    pub fn reset_all(&self) {
        let mut i = 0usize;
        while i < MAX_NUMA_NODES {
            self.reset_node(i);
            i = i.wrapping_add(1);
        }
    }
}

/// Singleton global des statistiques NUMA.
pub static NUMA_STATS: NumaStats = NumaStats::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_alloc() {
        let s = NumaStats::new_const();
        s.record_alloc(0, 4096);
        s.record_alloc(0, 4096);
        let n = s.node_stats(0);
        assert_eq!(n.allocs, 2);
        assert_eq!(n.bytes_alloc, 8192);
    }

    #[test]
    fn test_record_free() {
        let s = NumaStats::new_const();
        s.record_alloc(1, 8192);
        s.record_free(1, 4096);
        let n = s.node_stats(1);
        assert_eq!(n.frees, 1);
        assert_eq!(n.net_bytes(), 4096);
    }

    #[test]
    fn test_record_migration() {
        let s = NumaStats::new_const();
        s.record_migration(0, 2);
        assert_eq!(s.node_stats(0).migrations_out, 1);
        assert_eq!(s.node_stats(2).migrations_in, 1);
    }

    #[test]
    fn test_record_rw() {
        let s = NumaStats::new_const();
        s.record_read(0, 512);
        s.record_write(0, 1024);
        let n = s.node_stats(0);
        assert_eq!(n.read_ops, 1);
        assert_eq!(n.write_ops, 1);
        assert_eq!(n.read_bytes, 512);
        assert_eq!(n.write_bytes, 1024);
    }

    #[test]
    fn test_record_error() {
        let s = NumaStats::new_const();
        s.record_error(0);
        s.record_error(0);
        assert_eq!(s.node_stats(0).errors, 2);
        assert_eq!(s.total_errors(), 2);
    }

    #[test]
    fn test_out_of_range_node_noop() {
        let s = NumaStats::new_const();
        s.record_alloc(99, 4096); // doit être silencieux
        assert_eq!(s.total_allocs(), 0);
    }

    #[test]
    fn test_usage_ppt() {
        let s = NumaStats::new_const();
        s.record_alloc(0, 750);
        let n = s.node_stats(0);
        assert_eq!(n.usage_ppt(1000), 750);
    }

    #[test]
    fn test_least_loaded_node() {
        let s = NumaStats::new_const();
        s.record_alloc(0, 100_000);
        s.record_alloc(1, 1_000);
        assert_eq!(s.least_loaded_node(), 1);
    }

    #[test]
    fn test_most_loaded_node() {
        let s = NumaStats::new_const();
        s.record_alloc(0, 100_000);
        s.record_alloc(3, 50_000);
        assert_eq!(s.most_loaded_node(), 0);
    }

    #[test]
    fn test_imbalance_ppt_zero_when_equal() {
        let s = NumaStats::new_const();
        s.record_alloc(0, 4096);
        s.record_alloc(1, 4096);
        // Doit être 0 (chaque nœud a le même score)
        // Mais free=0 donc net = allocs : identiques → imbalance = 0
        assert_eq!(s.imbalance_ppt(), 0);
    }

    #[test]
    fn test_reset_node() {
        let s = NumaStats::new_const();
        s.record_alloc(2, 8192);
        s.reset_node(2);
        let n = s.node_stats(2);
        assert_eq!(n.allocs, 0);
        assert_eq!(n.bytes_alloc, 0);
    }

    #[test]
    fn test_reset_all() {
        let s = NumaStats::new_const();
        s.record_alloc(0, 100);
        s.record_alloc(1, 200);
        s.reset_all();
        assert_eq!(s.total_allocs(), 0);
    }

    #[test]
    fn test_all_nodes_returns_8() {
        let s = NumaStats::new_const();
        let v = s.all_nodes().unwrap();
        assert_eq!(v.len(), MAX_NUMA_NODES);
    }

    #[test]
    fn test_is_healthy_initial() {
        let s = NumaStats::new_const();
        assert!(s.is_healthy());
    }
}
