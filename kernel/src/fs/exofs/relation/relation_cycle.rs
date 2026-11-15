//! relation_cycle.rs — Iterative cycle detection in the ExoFS DAG
//!
//! Applied rules:
//!  - RECUR-01 : zero recursion — explicit stack DFS strictly required
//!  - OOM-02   : systematic try_reserve
//!  - ARITH-02 : checked arithmetic

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::relation_graph::RELATION_GRAPH;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Default maximum detection depth.
pub const CYCLE_DEFAULT_MAX_DEPTH: u32 = 128;

/// Maximum number of visited nodes during detection.
pub const CYCLE_DEFAULT_MAX_NODES: usize = 8192;

// ─────────────────────────────────────────────────────────────────────────────
// VisitState — DFS states
// ─────────────────────────────────────────────────────────────────────────────

/// Node visit state in cycle detection DFS.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VisitState {
    /// Not yet visited.
    Unvisited = 0,
    /// In current DFS stack (potential cycle).
    InStack = 1,
    /// Processing completed (no cycle reachable from this node).
    Done = 2,
}

// ─────────────────────────────────────────────────────────────────────────────
// CycleReport
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a cycle detection.
#[derive(Clone, Debug, Default)]
pub struct CycleReport {
    /// `true` if a cycle was detected.
    pub has_cycle: bool,
    /// Cycle nodes (empty if no cycle).
    pub cycle_path: Vec<BlobId>,
    /// Number of nodes explored before conclusion.
    pub n_explored: u32,
}

impl CycleReport {
    /// Length of detected cycle (0 if no cycle).
    pub fn cycle_len(&self) -> usize {
        self.cycle_path.len()
    }

    /// Report without cycle.
    pub fn no_cycle(n_explored: u32) -> Self {
        CycleReport {
            has_cycle: false,
            cycle_path: Vec::new(),
            n_explored,
        }
    }

    /// Report with cycle.
    pub fn with_cycle(path: Vec<BlobId>, n_explored: u32) -> Self {
        CycleReport {
            has_cycle: true,
            cycle_path: path,
            n_explored,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Iterative DFS — frame state
// ─────────────────────────────────────────────────────────────────────────────

/// A frame in the iterative DFS stack.
struct DfsFrame {
    node: BlobId,
    neighbor_idx: usize,
    neighbors: Vec<BlobId>,
    depth: u32,
}

impl DfsFrame {
    fn new(node: BlobId, depth: u32) -> ExofsResult<Self> {
        let neighbors = RELATION_GRAPH.get_neighbors(&node);
        Ok(DfsFrame {
            node,
            neighbor_idx: 0,
            neighbors,
            depth,
        })
    }

    /// Next neighbor not yet processed, or `None` if finished.
    fn next_neighbor(&mut self) -> Option<BlobId> {
        if self.neighbor_idx < self.neighbors.len() {
            let n = self.neighbors[self.neighbor_idx];
            self.neighbor_idx = self.neighbor_idx.wrapping_add(1);
            Some(n)
        } else {
            None
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationCycleDetector
// ─────────────────────────────────────────────────────────────────────────────

/// Cycle detector in the relation graph.
///
/// Uses an iterative DFS with tri-state coloring (Unvisited / InStack / Done).
/// RECUR-01: no recursive calls.
pub struct RelationCycleDetector;

impl RelationCycleDetector {
    /// Detects if a cycle is reachable from `start`.
    ///
    /// Iterative (RECUR-01): explicit `Vec<DfsFrame>` stack.
    pub fn detect_from(start: BlobId, max_depth: u32) -> ExofsResult<CycleReport> {
        let mut state: BTreeMap<[u8; 32], VisitState> = BTreeMap::new();
        let mut path: Vec<BlobId> = Vec::new(); // current path
        let mut stack: Vec<DfsFrame> = Vec::new();
        let mut n_explored: u32 = 0;

        // Initialization
        state.insert(*start.as_bytes(), VisitState::InStack);
        path.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        path.push(start);
        stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        stack.push(DfsFrame::new(start, 0)?);

        // Main loop (RECUR-01)
        while let Some(frame) = stack.last_mut() {
            n_explored = n_explored
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;

            if n_explored as usize > CYCLE_DEFAULT_MAX_NODES {
                // Safety limit — no cycle found in the window.
                return Ok(CycleReport::no_cycle(n_explored));
            }

            // Look for next unprocessed neighbor.
            let next = frame.next_neighbor();
            let cur_depth = frame.depth;

            match next {
                None => {
                    // All neighbors have been processed: this node is Done.
                    let node = frame.node;
                    stack.pop();
                    path.pop();
                    state.insert(*node.as_bytes(), VisitState::Done);
                }
                Some(nbr) => {
                    if cur_depth >= max_depth {
                        continue;
                    }

                    let nbr_key = *nbr.as_bytes();
                    let s = state
                        .get(&nbr_key)
                        .copied()
                        .unwrap_or(VisitState::Unvisited);

                    match s {
                        VisitState::InStack => {
                            // Cycle detected!
                            // Reconstitute cycle segment from `path`.
                            let mut cycle: Vec<BlobId> = Vec::new();
                            // Find node index in path.
                            let start_idx = path
                                .iter()
                                .position(|b| b.as_bytes() == &nbr_key)
                                .unwrap_or(0);
                            cycle
                                .try_reserve(path.len() - start_idx + 1)
                                .map_err(|_| ExofsError::NoMemory)?;
                            for n in &path[start_idx..] {
                                cycle.push(*n);
                            }
                            cycle.push(nbr); // back edge
                            return Ok(CycleReport::with_cycle(cycle, n_explored));
                        }
                        VisitState::Done => {
                            // Fully processed, no cycle this way.
                        }
                        VisitState::Unvisited => {
                            let new_depth =
                                cur_depth.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
                            state.insert(nbr_key, VisitState::InStack);
                            path.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                            path.push(nbr);
                            stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                            stack.push(DfsFrame::new(nbr, new_depth)?);
                        }
                    }
                }
            }
        }

        Ok(CycleReport::no_cycle(n_explored))
    }

    /// Runs detection from all nodes in the graph.
    ///
    /// Returns the first cycle found or a report without cycle.
    pub fn detect_global(max_depth: u32) -> ExofsResult<CycleReport> {
        let all_edges = RELATION_GRAPH.all_edges();
        let mut seen: BTreeMap<[u8; 32], ()> = BTreeMap::new();

        for (from, _) in all_edges {
            if seen.contains_key(from.as_bytes()) {
                continue;
            }
            seen.insert(*from.as_bytes(), ());
            let report = Self::detect_from(from, max_depth)?;
            if report.has_cycle {
                return Ok(report);
            }
        }

        Ok(CycleReport::no_cycle(seen.len() as u32))
    }

    /// Verifies that adding edge `from -> to` would not create a cycle.
    ///
    /// Returns `true` if addition is safe (no cycle).
    pub fn is_safe_to_add(from: &BlobId, to: &BlobId, max_depth: u32) -> ExofsResult<bool> {
        // A cycle would be created if `from` is already reachable from `to`.
        // Run a BFS from `to` and check if `from` is reachable.
        use alloc::collections::VecDeque;
        let mut visited: BTreeMap<[u8; 32], ()> = BTreeMap::new();
        let mut queue: VecDeque<(BlobId, u32)> = VecDeque::new();

        visited.insert(*to.as_bytes(), ());
        queue.push_back((*to, 0));

        // If from == to, immediate cycle.
        if from.as_bytes() == to.as_bytes() {
            return Ok(false);
        }

        while let Some((node, depth)) = queue.pop_front() {
            if node.as_bytes() == from.as_bytes() {
                return Ok(false); // cycle detected if edge were added
            }
            let next_depth = depth.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
            if next_depth > max_depth {
                continue;
            }

            for nbr in RELATION_GRAPH.get_neighbors(&node) {
                if !visited.contains_key(nbr.as_bytes()) {
                    visited.insert(*nbr.as_bytes(), ());
                    queue.push_back((nbr, next_depth));
                }
            }
        }

        Ok(true)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TopologicalSorter — iterative topological sort (Kahn)
// ─────────────────────────────────────────────────────────────────────────────

/// Topological sort result.
#[derive(Debug, Default)]
pub struct TopoResult {
    /// Topological order (from source towards sinks).
    pub order: Vec<BlobId>,
    /// `true` if a cycle prevented a complete sort.
    pub has_cycle: bool,
}

/// Kahn topological sort (iterative, RECUR-01).
///
/// Requires node and edge sets to be provided explicitly to
/// avoid any dependency on global mutable state.
pub struct TopologicalSorter;

impl TopologicalSorter {
    /// Topological sort from given nodes.
    ///
    /// - `nodes`: list of nodes to sort.
    /// - `edges`: list of edges (from, to).
    pub fn sort(nodes: &[BlobId], edges: &[(BlobId, BlobId)]) -> ExofsResult<TopoResult> {
        // Compute in-degree for each node.
        let mut in_degree: BTreeMap<[u8; 32], u32> = BTreeMap::new();
        let mut adj: BTreeMap<[u8; 32], Vec<BlobId>> = BTreeMap::new();

        for n in nodes {
            in_degree.entry(*n.as_bytes()).or_insert(0);
            adj.entry(*n.as_bytes()).or_insert_with(Vec::new);
        }

        for (from, to) in edges {
            if let Some(v) = adj.get_mut(from.as_bytes()) {
                v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                v.push(*to);
            }
            *in_degree.entry(*to.as_bytes()).or_insert(0) = in_degree
                .get(to.as_bytes())
                .copied()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;
        }

        // Starting queue: all nodes with in-degree 0.
        let mut queue: alloc::collections::VecDeque<BlobId> = alloc::collections::VecDeque::new();
        for (key, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(BlobId(*key));
            }
        }

        let mut order: Vec<BlobId> = Vec::new();

        while let Some(n) = queue.pop_front() {
            order.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            order.push(n);

            if let Some(neighbors) = adj.get(n.as_bytes()) {
                for &nbr in neighbors {
                    let d = in_degree.entry(*nbr.as_bytes()).or_insert(0);
                    *d = d.saturating_sub(1);
                    if *d == 0 {
                        queue.push_back(nbr);
                    }
                }
            }
        }

        let has_cycle = order.len() < nodes.len();
        Ok(TopoResult { order, has_cycle })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn b(n: u8) -> BlobId {
        BlobId([n; 32])
    }

    // ── CycleDetector ────────────────────────────────────────────────────────

    #[test]
    fn test_detect_isolated_no_cycle() {
        // Node without outgoing edges -> no cycle.
        let r = RelationCycleDetector::detect_from(b(99), 10).test_unwrap();
        assert!(!r.has_cycle);
    }

    #[test]
    fn test_cycle_report_no_cycle() {
        let r = CycleReport::no_cycle(5);
        assert!(!r.has_cycle);
        assert_eq!(r.cycle_len(), 0);
        assert_eq!(r.n_explored, 5);
    }

    #[test]
    fn test_cycle_report_with_cycle() {
        let path = alloc::vec![b(1), b(2), b(3), b(1)];
        let r = CycleReport::with_cycle(path, 3);
        assert!(r.has_cycle);
        assert_eq!(r.cycle_len(), 4);
    }

    #[test]
    fn test_is_safe_to_add_self_loop() {
        // from == to -> immediate cycle.
        let safe = RelationCycleDetector::is_safe_to_add(&b(10), &b(10), 5).test_unwrap();
        assert!(!safe);
    }

    #[test]
    fn test_is_safe_no_existing_arc() {
        // No edge between 20 and 21 -> safe to add.
        let safe = RelationCycleDetector::is_safe_to_add(&b(20), &b(21), 5).test_unwrap();
        assert!(safe);
    }

    #[test]
    fn test_detect_global_empty() {
        // Empty graph (or nodes without global edges to fictitious blobs).
        let r = RelationCycleDetector::detect_global(10).test_unwrap();
        // No cycle in empty graph.
        assert!(!r.has_cycle);
    }

    // ── TopologicalSorter ────────────────────────────────────────────────────

    #[test]
    fn test_topo_sort_chain() {
        // 1 → 2 → 3
        let nodes = alloc::vec![b(1), b(2), b(3)];
        let edges = alloc::vec![(b(1), b(2)), (b(2), b(3))];
        let res = TopologicalSorter::sort(&nodes, &edges).test_unwrap();
        assert!(!res.has_cycle);
        assert_eq!(res.order.len(), 3);
        // 1 must precede 2 and 3.
        let pos1 = res
            .order
            .iter()
            .position(|b| *b.as_bytes() == [1u8; 32])
            .test_unwrap();
        let pos2 = res
            .order
            .iter()
            .position(|b| *b.as_bytes() == [2u8; 32])
            .test_unwrap();
        let pos3 = res
            .order
            .iter()
            .position(|b| *b.as_bytes() == [3u8; 32])
            .test_unwrap();
        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
    }

    #[test]
    fn test_topo_sort_cycle() {
        // 1 → 2 → 1 (cycle)
        let nodes = alloc::vec![b(1), b(2)];
        let edges = alloc::vec![(b(1), b(2)), (b(2), b(1))];
        let res = TopologicalSorter::sort(&nodes, &edges).test_unwrap();
        assert!(res.has_cycle);
    }

    #[test]
    fn test_topo_sort_single_node() {
        let nodes = alloc::vec![b(5)];
        let edges: Vec<(BlobId, BlobId)> = alloc::vec![];
        let res = TopologicalSorter::sort(&nodes, &edges).test_unwrap();
        assert!(!res.has_cycle);
        assert_eq!(res.order.len(), 1);
    }

    #[test]
    fn test_topo_sort_diamond() {
        // 1 → 2, 1 → 3, 2 → 4, 3 → 4
        let nodes = alloc::vec![b(1), b(2), b(3), b(4)];
        let edges = alloc::vec![(b(1), b(2)), (b(1), b(3)), (b(2), b(4)), (b(3), b(4))];
        let res = TopologicalSorter::sort(&nodes, &edges).test_unwrap();
        assert!(!res.has_cycle);
        assert_eq!(res.order.len(), 4);
    }
}
