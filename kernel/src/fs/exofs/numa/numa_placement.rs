// SPDX-License-Identifier: MIT
// ExoFS NUMA — Stratégies de placement de blobs sur les nœuds NUMA
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use super::numa_affinity::{NumaNodeId, MAX_NUMA_NODES};
use super::numa_stats::NUMA_STATS;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

// ─── PlacementStrategy ────────────────────────────────────────────────────────

/// Stratégie de placement des blobs sur les nœuds NUMA.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementStrategy {
    /// Distribution cyclique entre nœuds actifs.
    RoundRobin = 0,
    /// Nœud de la tâche courante (hash de l'id blob).
    LocalFirst = 1,
    /// Nœud avec le moins d'octets alloués (NUMA_STATS).
    LeastUsed = 2,
    /// Nœud fixe imposé par le gestionnaire.
    Pinned = 3,
    /// Hash cohérent sur l'identifiant du blob.
    ContentHash = 4,
}

impl PlacementStrategy {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::RoundRobin,
            1 => Self::LocalFirst,
            2 => Self::LeastUsed,
            3 => Self::Pinned,
            4 => Self::ContentHash,
            _ => Self::RoundRobin,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::RoundRobin => "round-robin",
            Self::LocalFirst => "local-first",
            Self::LeastUsed => "least-used",
            Self::Pinned => "pinned",
            Self::ContentHash => "content-hash",
        }
    }
}

// ─── PlacementHint ────────────────────────────────────────────────────────────

/// Indice de placement transmis à `preferred_node`.
#[derive(Clone, Copy, Debug)]
pub struct PlacementHint {
    /// Id optionnel du blob à placer.
    pub blob_id: Option<BlobId>,
    /// CPU demandeur, pour LocalFirst.
    pub cpu_id: Option<u32>,
    /// Nœud forcé, pour Pinned.
    pub pinned_node: Option<NumaNodeId>,
    /// Taille du blob en octets.
    pub data_bytes: u64,
}

impl PlacementHint {
    pub const fn simple(blob_id: Option<BlobId>) -> Self {
        Self {
            blob_id,
            cpu_id: None,
            pinned_node: None,
            data_bytes: 0,
        }
    }
    pub fn with_cpu(mut self, cpu: u32) -> Self {
        self.cpu_id = Some(cpu);
        self
    }
    pub fn with_pin(mut self, node: NumaNodeId) -> Self {
        self.pinned_node = Some(node);
        self
    }
    pub fn with_size(mut self, bytes: u64) -> Self {
        self.data_bytes = bytes;
        self
    }
}

// ─── PlacementResult ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct PlacementResult {
    pub node: NumaNodeId,
    pub strategy: PlacementStrategy,
    /// Score de charge du nœud sélectionné au moment du placement.
    pub load_score: u64,
}

// ─── NumaPlacement ───────────────────────────────────────────────────────────

/// Moteur de placement NUMA des blobs.
pub struct NumaPlacement {
    strategy: AtomicU8,
    n_nodes: AtomicU8,
    rr_counter: AtomicU64,
    pinned_node: AtomicU8,
    /// Compteur de placements total.
    place_count: AtomicU64,
    /// Compteur de placements refusés (node invalide).
    place_errors: AtomicU64,
}

impl NumaPlacement {
    pub const fn new_const() -> Self {
        Self {
            strategy: AtomicU8::new(PlacementStrategy::RoundRobin as u8),
            n_nodes: AtomicU8::new(1),
            rr_counter: AtomicU64::new(0),
            pinned_node: AtomicU8::new(0),
            place_count: AtomicU64::new(0),
            place_errors: AtomicU64::new(0),
        }
    }

    /// Initialise le moteur de placement.
    pub fn init(&self, n_nodes: u8, strategy: PlacementStrategy) -> ExofsResult<()> {
        if n_nodes == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        let n = n_nodes.min(MAX_NUMA_NODES as u8);
        self.n_nodes.store(n, Ordering::Relaxed);
        self.strategy.store(strategy as u8, Ordering::Relaxed);
        self.rr_counter.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// Fixe le nœud de l'épinglage (stratégie Pinned).
    pub fn set_pinned_node(&self, node: NumaNodeId) -> ExofsResult<()> {
        if !node.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        self.pinned_node.store(node.0, Ordering::Relaxed);
        Ok(())
    }

    pub fn n_nodes(&self) -> usize {
        self.n_nodes.load(Ordering::Relaxed) as usize
    }
    pub fn strategy(&self) -> PlacementStrategy {
        PlacementStrategy::from_u8(self.strategy.load(Ordering::Relaxed))
    }
    pub fn place_count(&self) -> u64 {
        self.place_count.load(Ordering::Relaxed)
    }
    pub fn place_errors(&self) -> u64 {
        self.place_errors.load(Ordering::Relaxed)
    }

    // ── Placement ─────────────────────────────────────────────────────────────

    /// Retourne le nœud préféré pour un placement donné.
    pub fn preferred_node(&self, hint: &PlacementHint) -> ExofsResult<PlacementResult> {
        let n = self.n_nodes.load(Ordering::Relaxed) as usize;
        if n == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if n == 1 {
            self.place_count.fetch_add(1, Ordering::Relaxed);
            let score = NUMA_STATS.node_stats(0).load_score();
            return Ok(PlacementResult {
                node: NumaNodeId(0),
                strategy: self.strategy(),
                load_score: score,
            });
        }

        let strat = PlacementStrategy::from_u8(self.strategy.load(Ordering::Relaxed));
        let node_idx = match strat {
            PlacementStrategy::RoundRobin => self._round_robin(n),
            PlacementStrategy::LocalFirst => self._local_first(hint, n),
            PlacementStrategy::LeastUsed => self._least_used(n),
            PlacementStrategy::Pinned => self._pinned(n),
            PlacementStrategy::ContentHash => self._content_hash(hint, n),
        };

        if node_idx >= n {
            self.place_errors.fetch_add(1, Ordering::Relaxed);
            return Err(ExofsError::InvalidArgument);
        }
        self.place_count.fetch_add(1, Ordering::Relaxed);
        let score = NUMA_STATS.node_stats(node_idx).load_score();
        Ok(PlacementResult {
            node: NumaNodeId(node_idx as u8),
            strategy: strat,
            load_score: score,
        })
    }

    /// Retourne le nœud préféré (simple, sans résultat étendu).
    pub fn node_for(&self, hint: &PlacementHint) -> usize {
        self.preferred_node(hint).map(|r| r.node.idx()).unwrap_or(0)
    }

    // ── Stratégies privées ────────────────────────────────────────────────────

    fn _round_robin(&self, n: usize) -> usize {
        let c = self.rr_counter.fetch_add(1, Ordering::Relaxed);
        // ARITH-02 : wrapping_add déjà fait par fetch_add ; modulo safe car n>0
        (c as usize).wrapping_rem(n)
    }

    fn _local_first(&self, hint: &PlacementHint, n: usize) -> usize {
        // Hash du CPU sur les nœuds
        if let Some(cpu) = hint.cpu_id {
            return (cpu as usize).wrapping_rem(n);
        }
        // Fallback : hash de BlobId
        if let Some(ref bid) = hint.blob_id {
            return self._blob_hash(bid, n);
        }
        // Fallback : round-robin
        self._round_robin(n)
    }

    fn _least_used(&self, n: usize) -> usize {
        let mut best = 0usize;
        let mut best_score = u64::MAX;
        let mut i = 0usize;
        while i < n {
            let score = NUMA_STATS.node_stats(i).load_score();
            if score < best_score {
                best_score = score;
                best = i;
            }
            i = i.wrapping_add(1);
        }
        best
    }

    fn _pinned(&self, n: usize) -> usize {
        let p = self.pinned_node.load(Ordering::Relaxed) as usize;
        if p < n {
            p
        } else {
            0
        }
    }

    fn _content_hash(&self, hint: &PlacementHint, n: usize) -> usize {
        if let Some(ref bid) = hint.blob_id {
            return self._blob_hash(bid, n);
        }
        self._round_robin(n)
    }

    /// Hash stable d'un BlobId sur [0, n[ (ARITH-02 : wrapping_rem).
    fn _blob_hash(&self, bid: &BlobId, n: usize) -> usize {
        let b = bid.as_bytes();
        // Prendre les 8 premiers octets comme u64
        let h = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        (h as usize).wrapping_rem(n)
    }

    // ── Statistiques ──────────────────────────────────────────────────────────

    /// Distribution des placements par nœud (RECUR-01).
    pub fn node_distribution(&self, _n_samples: usize) -> ExofsResult<Vec<(NumaNodeId, u64)>> {
        let n = self.n_nodes.load(Ordering::Relaxed) as usize;
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        // On retourne la charge actuelle de chaque nœud comme proxy de la distribution
        let mut i = 0usize;
        while i < n {
            let score = NUMA_STATS.node_stats(i).load_score();
            v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            v.push((NumaNodeId(i as u8), score));
            i = i.wrapping_add(1);
        }
        Ok(v)
    }

    /// Vérifie que le nœud `node` est valide pour ce moteur.
    pub fn validate_node(&self, node: NumaNodeId) -> ExofsResult<()> {
        if !node.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        let n = self.n_nodes.load(Ordering::Relaxed) as usize;
        if node.idx() >= n {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }

    /// Réinitialise les compteurs (pas la stratégie).
    pub fn reset_counters(&self) {
        self.rr_counter.store(0, Ordering::Relaxed);
        self.place_count.store(0, Ordering::Relaxed);
        self.place_errors.store(0, Ordering::Relaxed);
    }
}

/// Singleton global du moteur de placement.
pub static NUMA_PLACEMENT: NumaPlacement = NumaPlacement::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn hint() -> PlacementHint {
        PlacementHint::simple(None)
    }

    fn init_p(n: u8, s: PlacementStrategy) -> NumaPlacement {
        let p = NumaPlacement::new_const();
        p.init(n, s).unwrap();
        p
    }

    #[test]
    fn test_round_robin_cycles() {
        let p = init_p(4, PlacementStrategy::RoundRobin);
        let mut nodes = [0usize; 8];
        let mut i = 0usize;
        while i < 8 {
            nodes[i] = p.node_for(&hint());
            i = i.wrapping_add(1);
        }
        // nodes[0]=0, nodes[1]=1, nodes[2]=2, nodes[3]=3, nodes[4]=0 …
        assert_eq!(nodes[0], 0);
        assert_eq!(nodes[1], 1);
        assert_eq!(nodes[4], 0);
    }

    #[test]
    fn test_single_node_always_zero() {
        let p = init_p(1, PlacementStrategy::RoundRobin);
        assert_eq!(p.node_for(&hint()), 0);
        assert_eq!(p.node_for(&hint()), 0);
    }

    #[test]
    fn test_pinned_strategy() {
        let p = init_p(4, PlacementStrategy::Pinned);
        p.set_pinned_node(NumaNodeId(2)).unwrap();
        let mut i = 0usize;
        while i < 5 {
            assert_eq!(p.node_for(&hint()), 2);
            i = i.wrapping_add(1);
        }
    }

    #[test]
    fn test_local_first_with_cpu() {
        let p = init_p(4, PlacementStrategy::LocalFirst);
        let h = hint().with_cpu(6); // 6 % 4 = 2
        assert_eq!(p.node_for(&h), 2);
    }

    #[test]
    fn test_content_hash_stable() {
        let p = init_p(4, PlacementStrategy::ContentHash);
        let bid = BlobId([
            1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let h = PlacementHint::simple(Some(bid));
        let node1 = p.node_for(&h);
        let node2 = p.node_for(&h);
        assert_eq!(node1, node2); // stable
    }

    #[test]
    fn test_least_used_prefers_empty() {
        let p = init_p(4, PlacementStrategy::LeastUsed);
        // Node 1 très chargé
        NUMA_STATS.record_alloc(0, 100_000);
        NUMA_STATS.record_alloc(2, 100_000);
        NUMA_STATS.record_alloc(3, 100_000);
        // Node 1 libre → devrait être sélectionné
        let node = p.node_for(&hint());
        assert_eq!(node, 1);
        NUMA_STATS.reset_all();
    }

    #[test]
    fn test_init_zero_nodes_error() {
        let p = NumaPlacement::new_const();
        assert!(p.init(0, PlacementStrategy::RoundRobin).is_err());
    }

    #[test]
    fn test_set_pinned_invalid_node_error() {
        let p = NumaPlacement::new_const();
        assert!(p.set_pinned_node(NumaNodeId(9)).is_err());
    }

    #[test]
    fn test_place_count_increments() {
        let p = init_p(2, PlacementStrategy::RoundRobin);
        p.node_for(&hint());
        p.node_for(&hint());
        assert_eq!(p.place_count(), 2);
    }

    #[test]
    fn test_reset_counters() {
        let p = init_p(2, PlacementStrategy::RoundRobin);
        p.node_for(&hint());
        p.reset_counters();
        assert_eq!(p.place_count(), 0);
        // RR repart de 0
        assert_eq!(p.node_for(&hint()), 0);
    }

    #[test]
    fn test_validate_node_ok() {
        let p = init_p(4, PlacementStrategy::RoundRobin);
        assert!(p.validate_node(NumaNodeId(3)).is_ok());
    }

    #[test]
    fn test_validate_node_out_of_range() {
        let p = init_p(2, PlacementStrategy::RoundRobin);
        assert!(p.validate_node(NumaNodeId(3)).is_err());
    }

    #[test]
    fn test_strategy_name() {
        assert_eq!(PlacementStrategy::RoundRobin.name(), "round-robin");
        assert_eq!(PlacementStrategy::LeastUsed.name(), "least-used");
        assert_eq!(PlacementStrategy::ContentHash.name(), "content-hash");
    }

    #[test]
    fn test_node_distribution_len() {
        let p = init_p(3, PlacementStrategy::RoundRobin);
        let d = p.node_distribution(10).unwrap();
        assert_eq!(d.len(), 3);
    }
}
