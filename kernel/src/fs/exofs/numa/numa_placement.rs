// SPDX-License-Identifier: GPL-3.0
// ExoFS NUMA — Blob placement strategies on NUMA nodes
// ≥400L, ExofsError only, RECUR-01/OOM-02/ARITH-02

use super::numa_affinity::{NumaNodeId, AFFINITY_MAP, CpuId, MAX_NUMA_NODES};
use super::numa_stats::NUMA_STATS;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

// ─── PlacementStrategy ────────────────────────────────────────────────────────

/// Placement strategy for blobs on NUMA nodes.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementStrategy {
    /// Cyclic distribution across active nodes.
    RoundRobin = 0,
    /// Current task's node (hash of blob id).
    LocalFirst = 1,
    /// Node with least allocated bytes (NUMA_STATS).
    LeastUsed = 2,
    /// Fixed node imposed by manager.
    Pinned = 3,
    /// Consistent hash on the blob identifier.
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

/// Placement hint passed to `preferred_node`.
#[derive(Clone, Copy, Debug)]
pub struct PlacementHint {
    /// Optional id of the blob to place.
    pub blob_id: Option<BlobId>,
    /// Requesting CPU, for LocalFirst.
    pub cpu_id: Option<u32>,
    /// Forced node, for Pinned.
    pub pinned_node: Option<NumaNodeId>,
    /// Blob size in bytes.
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
    /// Load score of selected node at placement time.
    pub load_score: u64,
}

// ─── NumaPlacement ───────────────────────────────────────────────────────────

/// NUMA placement engine for blobs.
pub struct NumaPlacement {
    strategy: AtomicU8,
    n_nodes: AtomicU8,
    rr_counter: AtomicU64,
    pinned_node: AtomicU8,
    /// Total placement counter.
    place_count: AtomicU64,
    /// Denied placement counter (invalid node).
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

    /// Initializes the placement engine.
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

    /// Sets pinned node (Pinned strategy).
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

    /// Returns the preferred node for a given placement.
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

    /// Returns the preferred node (simple, without extended result).
    pub fn node_for(&self, hint: &PlacementHint) -> usize {
        self.preferred_node(hint).map(|r| r.node.idx()).unwrap_or(0)
    }

    // ── Private strategies ────────────────────────────────────────────────────

    fn _round_robin(&self, n: usize) -> usize {
        let c = self.rr_counter.fetch_add(1, Ordering::Relaxed);
        // ARITH-02: wrapping_add already performed by fetch_add; modulo safe as n>0
        (c as usize).wrapping_rem(n)
    }

    fn _local_first(&self, hint: &PlacementHint, n: usize) -> usize {
        // Resolve CPU to its actual NUMA node via AFFINITY_MAP
        if let Some(cpu) = hint.cpu_id {
            if let Some(node) = AFFINITY_MAP.node_of_cpu(CpuId(cpu as u32)) {
                let idx = node.idx();
                if idx < n {
                    return idx;
                }
            }
            return (cpu as usize).wrapping_rem(n);
        }
        // Fallback: hash of BlobId
        if let Some(ref bid) = hint.blob_id {
            return self._blob_hash(bid, n);
        }
        // Fallback: round-robin
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

    /// Stable hash of a BlobId on [0, n[ (ARITH-02: wrapping_rem).
    fn _blob_hash(&self, bid: &BlobId, n: usize) -> usize {
        let b = bid.as_bytes();
        // Take first 8 bytes as u64
        let h = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        (h as usize).wrapping_rem(n)
    }

    // ── Statistics ────────────────────────────────────────────────────────────

    /// Placement distribution per node (RECUR-01).
    pub fn node_distribution(&self, _n_samples: usize) -> ExofsResult<Vec<(NumaNodeId, u64)>> {
        let n = self.n_nodes.load(Ordering::Relaxed) as usize;
        let mut v = Vec::new();
        v.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        // Return current load of each node as distribution proxy
        let mut i = 0usize;
        while i < n {
            let score = NUMA_STATS.node_stats(i).load_score();
            v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            v.push((NumaNodeId(i as u8), score));
            i = i.wrapping_add(1);
        }
        Ok(v)
    }

    /// Checks that node `node` is valid for this engine.
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

    /// Resets counters (not strategy).
    pub fn reset_counters(&self) {
        self.rr_counter.store(0, Ordering::Relaxed);
        self.place_count.store(0, Ordering::Relaxed);
        self.place_errors.store(0, Ordering::Relaxed);
    }
}

/// Global singleton of the placement engine.
pub static NUMA_PLACEMENT: NumaPlacement = NumaPlacement::new_const();

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn hint() -> PlacementHint {
        PlacementHint::simple(None)
    }

    fn init_p(n: u8, s: PlacementStrategy) -> NumaPlacement {
        let p = NumaPlacement::new_const();
        p.init(n, s).test_unwrap();
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
        p.set_pinned_node(NumaNodeId(2)).test_unwrap();
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
        // Node 1 heavily loaded
        NUMA_STATS.record_alloc(0, 100_000);
        NUMA_STATS.record_alloc(2, 100_000);
        NUMA_STATS.record_alloc(3, 100_000);
        // Node 1 free -> should be selected
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
        // RR restarts from 0
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
        let d = p.node_distribution(10).test_unwrap();
        assert_eq!(d.len(), 3);
    }
}
