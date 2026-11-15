//! relation_walker.rs — Iterative traversal of the ExoFS relation graph
//!
//! Applied rules:
//!  - RECUR-01: zero recursion — BFS/DFS with explicit queue/stack
//!  - OOM-02: try_reserve before any push
//!  - ARITH-02: checked arithmetic

extern crate alloc;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use super::relation_graph::RELATION_GRAPH;
use super::relation_type::RelationKind;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Default maximum traversal depth.
pub const WALKER_DEFAULT_MAX_DEPTH: u32 = 64;

/// Default maximum number of visited nodes.
pub const WALKER_DEFAULT_MAX_NODES: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// WalkResult
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a graph traversal.
#[derive(Clone, Debug, Default)]
pub struct WalkResult {
    /// All visited blobs (discovery order).
    pub visited: Vec<BlobId>,
    /// Maximum depth reached.
    pub depth_reached: u32,
    /// Number of edges traversed.
    pub n_edges_traversed: u32,
    /// `true` if the traversal was interrupted (limit exceeded).
    pub truncated: bool,
}

impl WalkResult {
    /// `true` if the node was visited.
    pub fn contains(&self, blob: &BlobId) -> bool {
        self.visited.iter().any(|b| b.as_bytes() == blob.as_bytes())
    }

    /// Number of visited nodes.
    pub fn n_visited(&self) -> usize {
        self.visited.len()
    }

    /// `true` if the result is empty.
    pub fn is_empty(&self) -> bool {
        self.visited.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WalkOptions
// ─────────────────────────────────────────────────────────────────────────────

/// Configurable options for a traversal.
#[derive(Clone, Debug)]
pub struct WalkOptions {
    /// Maximum depth.
    pub max_depth: u32,
    /// Maximum number of nodes to visit.
    pub max_nodes: usize,
    /// Filter by kind (no filter if `None`).
    pub kind_filter: Option<RelationKind>,
    /// Include self loops (edge from A to A).
    pub include_self_loops: bool,
}

impl Default for WalkOptions {
    fn default() -> Self {
        WalkOptions {
            max_depth: WALKER_DEFAULT_MAX_DEPTH,
            max_nodes: WALKER_DEFAULT_MAX_NODES,
            kind_filter: None,
            include_self_loops: false,
        }
    }
}

impl WalkOptions {
    /// Options for breadth-first traversal (default).
    pub fn bfs_default() -> Self {
        Self::default()
    }

    /// Restricted options (shallow depth, few nodes).
    pub fn shallow(depth: u32) -> Self {
        WalkOptions {
            max_depth: depth,
            max_nodes: 256,
            ..Default::default()
        }
    }

    /// Options with filter on relation kind.
    pub fn with_kind(kind: RelationKind) -> Self {
        WalkOptions {
            kind_filter: Some(kind),
            ..Default::default()
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationWalker
// ─────────────────────────────────────────────────────────────────────────────

/// Iterative traversal of the relation graph.
///
/// Supports BFS (breadth-first) and DFS (iterative depth-first).
/// RECUR-01: no recursion.
pub struct RelationWalker {
    pub max_depth: u32,
    pub options: WalkOptions,
}

impl RelationWalker {
    /// Creates a walker with the given maximum depth.
    pub fn new(max_depth: u32) -> Self {
        RelationWalker {
            max_depth,
            options: WalkOptions {
                max_depth,
                ..Default::default()
            },
        }
    }

    /// Creates a walker with full options.
    pub fn with_options(options: WalkOptions) -> Self {
        let max_depth = options.max_depth;
        RelationWalker { max_depth, options }
    }

    // ── BFS ──────────────────────────────────────────────────────────────────

    /// BFS starting from `start`.
    ///
    /// Iterative (RECUR-01): uses `VecDeque` as queue.
    pub fn bfs(&self, start: &BlobId) -> ExofsResult<WalkResult> {
        let mut visited: BTreeMap<[u8; 32], u32> = BTreeMap::new();
        let mut queue: VecDeque<(BlobId, u32)> = VecDeque::new();
        let mut result = WalkResult::default();

        visited.insert(*start.as_bytes(), 0);
        queue.push_back((*start, 0));

        while let Some((node, depth)) = queue.pop_front() {
            result
                .visited
                .try_reserve(1)
                .map_err(|_| ExofsError::NoMemory)?;
            result.visited.push(node);
            if depth > result.depth_reached {
                result.depth_reached = depth;
            }

            if result.visited.len() >= self.options.max_nodes {
                result.truncated = true;
                break;
            }

            let next_depth = depth.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
            if next_depth > self.options.max_depth {
                continue;
            }

            let neighbors = self.collect_neighbors(&node);
            for nbr in neighbors {
                if !self.options.include_self_loops && nbr.as_bytes() == node.as_bytes() {
                    continue;
                }
                if !visited.contains_key(nbr.as_bytes()) {
                    visited.insert(*nbr.as_bytes(), next_depth);
                    queue.push_back((nbr, next_depth));
                    result.n_edges_traversed = result
                        .n_edges_traversed
                        .checked_add(1)
                        .ok_or(ExofsError::OffsetOverflow)?;
                }
            }
        }

        Ok(result)
    }

    // ── DFS ──────────────────────────────────────────────────────────────────

    /// DFS starting from `start`.
    ///
    /// Iterative (RECUR-01): uses `Vec` as explicit stack.
    pub fn dfs(&self, start: &BlobId) -> ExofsResult<WalkResult> {
        let mut visited: BTreeMap<[u8; 32], ()> = BTreeMap::new();
        let mut stack: Vec<(BlobId, u32)> = Vec::new();
        let mut result = WalkResult::default();

        stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        stack.push((*start, 0));

        while let Some((node, depth)) = stack.pop() {
            let key = *node.as_bytes();
            if visited.contains_key(&key) {
                continue;
            }

            visited.insert(key, ());

            result
                .visited
                .try_reserve(1)
                .map_err(|_| ExofsError::NoMemory)?;
            result.visited.push(node);
            if depth > result.depth_reached {
                result.depth_reached = depth;
            }

            if result.visited.len() >= self.options.max_nodes {
                result.truncated = true;
                break;
            }

            let next_depth = depth.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
            if next_depth > self.options.max_depth {
                continue;
            }

            let neighbors = self.collect_neighbors(&node);
            // Reverse to preserve natural order in the stack.
            for nbr in neighbors.into_iter().rev() {
                if !visited.contains_key(nbr.as_bytes()) {
                    stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    stack.push((nbr, next_depth));
                    result.n_edges_traversed = result
                        .n_edges_traversed
                        .checked_add(1)
                        .ok_or(ExofsError::OffsetOverflow)?;
                }
            }
        }

        Ok(result)
    }

    // ── Path finding ──────────────────────────────────────────────────────────

    /// BFS up to a target node. Returns the path or `None` if unreachable.
    ///
    /// The returned path includes `start` and `target`.
    pub fn find_path(&self, start: &BlobId, target: &BlobId) -> ExofsResult<Option<Vec<BlobId>>> {
        // parent[node] = parent in the BFS tree.
        let mut parent: BTreeMap<[u8; 32], [u8; 32]> = BTreeMap::new();
        let mut queue: VecDeque<(BlobId, u32)> = VecDeque::new();
        let sentinel = [0xFFu8; 32]; // root without parent

        parent.insert(*start.as_bytes(), sentinel);
        queue.push_back((*start, 0));

        while let Some((node, depth)) = queue.pop_front() {
            if node.as_bytes() == target.as_bytes() {
                // Path reconstruction (RECUR-01: while loop)
                let mut path: Vec<BlobId> = Vec::new();
                let mut cur = *target.as_bytes();
                loop {
                    path.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    path.push(BlobId(cur));
                    let p = parent[&cur];
                    if p == sentinel {
                        break;
                    }
                    cur = p;
                }
                path.reverse();
                return Ok(Some(path));
            }

            let next_depth = depth.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
            if next_depth > self.options.max_depth {
                continue;
            }

            for nbr in self.collect_neighbors(&node) {
                if !parent.contains_key(nbr.as_bytes()) {
                    parent.insert(*nbr.as_bytes(), *node.as_bytes());
                    queue.push_back((nbr, next_depth));
                }
            }
        }

        Ok(None)
    }

    /// `true` if `target` is reachable from `start`.
    pub fn is_reachable(&self, start: &BlobId, target: &BlobId) -> ExofsResult<bool> {
        // Optimization: if start == target, it is immediately true.
        if start.as_bytes() == target.as_bytes() {
            return Ok(true);
        }
        Ok(self.find_path(start, target)?.is_some())
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn collect_neighbors(&self, node: &BlobId) -> Vec<BlobId> {
        match self.options.kind_filter {
            None => RELATION_GRAPH.get_neighbors(node),
            Some(k) => RELATION_GRAPH.get_neighbors_by_kind(node, k),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationStepWalker — step-by-step traversal
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a single step of the step-by-step walker.
#[derive(Debug)]
pub enum StepResult {
    /// A new node has been discovered.
    Visited(BlobId),
    /// Traversal finished (no remaining nodes).
    Done,
    /// Fatal error.
    Error(ExofsError),
}

/// Step-by-step walker: each call to `step()` discovers one node.
///
/// Useful for interleaving discovery with other processing.
pub struct RelationStepWalker {
    options: WalkOptions,
    queue: VecDeque<(BlobId, u32)>,
    visited: BTreeMap<[u8; 32], u32>,
    pub result: WalkResult,
}

impl RelationStepWalker {
    /// Creates a step-by-step walker starting from `start`.
    pub fn new(start: BlobId, options: WalkOptions) -> ExofsResult<Self> {
        let mut visited: BTreeMap<[u8; 32], u32> = BTreeMap::new();
        let mut queue: VecDeque<(BlobId, u32)> = VecDeque::new();
        visited.insert(*start.as_bytes(), 0);
        queue.push_back((start, 0));
        Ok(RelationStepWalker {
            options,
            queue,
            visited,
            result: WalkResult::default(),
        })
    }

    /// Advances by one node in the BFS.
    pub fn step(&mut self) -> StepResult {
        let (node, depth) = match self.queue.pop_front() {
            None => return StepResult::Done,
            Some(e) => e,
        };

        if self.result.visited.try_reserve(1).is_err() {
            return StepResult::Error(ExofsError::NoMemory);
        }
        self.result.visited.push(node);
        if depth > self.result.depth_reached {
            self.result.depth_reached = depth;
        }

        if self.result.visited.len() >= self.options.max_nodes {
            self.result.truncated = true;
            return StepResult::Done;
        }

        let next_depth = match depth.checked_add(1) {
            None => return StepResult::Error(ExofsError::OffsetOverflow),
            Some(d) => d,
        };

        if next_depth <= self.options.max_depth {
            let neighbors = match self.options.kind_filter {
                None => RELATION_GRAPH.get_neighbors(&node),
                Some(k) => RELATION_GRAPH.get_neighbors_by_kind(&node, k),
            };
            for nbr in neighbors {
                if !self.visited.contains_key(nbr.as_bytes()) {
                    self.visited.insert(*nbr.as_bytes(), next_depth);
                    if self.queue.try_reserve(1).is_err() {
                        return StepResult::Error(ExofsError::NoMemory);
                    }
                    self.queue.push_back((nbr, next_depth));
                }
            }
        }

        StepResult::Visited(node)
    }

    /// Completes the full traversal and returns the result.
    pub fn finish(mut self) -> WalkResult {
        loop {
            match self.step() {
                StepResult::Done | StepResult::Error(_) => break,
                StepResult::Visited(_) => {}
            }
        }
        self.result
    }

    /// `true` if the queue is empty (traversal completed).
    pub fn is_done(&self) -> bool {
        self.queue.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utility functions
// ─────────────────────────────────────────────────────────────────────────────

/// All blobs reachable from `start` without depth limit.
pub fn reachable_from_all(start: &BlobId) -> ExofsResult<Vec<BlobId>> {
    let w = RelationWalker::new(u32::MAX / 2);
    Ok(w.bfs(start)?.visited)
}

/// Hop distance between two blobs.
/// Returns `None` if `target` is unreachable.
pub fn hop_distance(start: &BlobId, target: &BlobId) -> ExofsResult<Option<u32>> {
    if start.as_bytes() == target.as_bytes() {
        return Ok(Some(0));
    }
    let w = RelationWalker::new(WALKER_DEFAULT_MAX_DEPTH);
    match w.find_path(start, target)? {
        None => Ok(None),
        Some(path) => {
            let n = path.len().saturating_sub(1) as u32;
            Ok(Some(n))
        }
    }
}

/// Collects all blobs at exactly `depth` hops from `start`.
pub fn nodes_at_depth(start: &BlobId, depth: u32) -> ExofsResult<Vec<BlobId>> {
    let mut current_layer: Vec<BlobId> = Vec::new();
    current_layer
        .try_reserve(1)
        .map_err(|_| ExofsError::NoMemory)?;
    current_layer.push(*start);

    let mut visited: BTreeMap<[u8; 32], ()> = BTreeMap::new();
    visited.insert(*start.as_bytes(), ());

    let mut d = 0u32;
    while d < depth {
        let mut next_layer: Vec<BlobId> = Vec::new();
        for node in &current_layer {
            let neighbors = RELATION_GRAPH.get_neighbors(node);
            for nbr in neighbors {
                if !visited.contains_key(nbr.as_bytes()) {
                    visited.insert(*nbr.as_bytes(), ());
                    next_layer
                        .try_reserve(1)
                        .map_err(|_| ExofsError::NoMemory)?;
                    next_layer.push(nbr);
                }
            }
        }
        current_layer = next_layer;
        d = d.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
    }

    Ok(current_layer)
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

    #[test]
    fn test_walk_result_contains() {
        let mut r = WalkResult::default();
        r.visited.push(b(5));
        assert!(r.contains(&b(5)));
        assert!(!r.contains(&b(6)));
    }

    #[test]
    fn test_walk_result_empty() {
        let r = WalkResult::default();
        assert!(r.is_empty());
        assert_eq!(r.n_visited(), 0);
    }

    #[test]
    fn test_bfs_isolated_node() {
        let w = RelationWalker::new(10);
        let res = w.bfs(&b(99)).test_unwrap();
        // No edges from this node -> only it is visited.
        assert_eq!(res.n_visited(), 1);
        assert_eq!(res.depth_reached, 0);
        assert!(!res.truncated);
    }

    #[test]
    fn test_dfs_isolated_node() {
        let w = RelationWalker::new(5);
        let res = w.dfs(&b(70)).test_unwrap();
        assert_eq!(res.n_visited(), 1);
    }

    #[test]
    fn test_find_path_not_reachable() {
        let w = RelationWalker::new(5);
        let res = w.find_path(&b(50), &b(51)).test_unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn test_find_path_self() {
        let w = RelationWalker::new(5);
        let res = w.find_path(&b(30), &b(30)).test_unwrap();
        assert!(res.is_some());
        let path = res.test_unwrap();
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].as_bytes(), b(30).as_bytes());
    }

    #[test]
    fn test_is_reachable_self() {
        let w = RelationWalker::new(5);
        assert!(w.is_reachable(&b(80), &b(80)).test_unwrap());
    }

    #[test]
    fn test_is_reachable_no_arc() {
        let w = RelationWalker::new(5);
        assert!(!w.is_reachable(&b(10), &b(11)).test_unwrap());
    }

    #[test]
    fn test_max_nodes_truncation() {
        let opts = WalkOptions {
            max_nodes: 1,
            ..Default::default()
        };
        let w = RelationWalker::with_options(opts);
        let res = w.bfs(&b(40)).test_unwrap();
        assert!(res.truncated);
        assert_eq!(res.n_visited(), 1);
    }

    #[test]
    fn test_step_walker_visits_start() {
        let opts = WalkOptions::default();
        let mut sw = RelationStepWalker::new(b(20), opts).test_unwrap();
        match sw.step() {
            StepResult::Visited(blob) => {
                assert_eq!(blob.as_bytes(), b(20).as_bytes());
            }
            other => panic!("expected Visited, got {:?}", other),
        }
        match sw.step() {
            StepResult::Done => {}
            other => panic!("expected Done, got {:?}", other),
        }
    }

    #[test]
    fn test_step_walker_is_done() {
        let opts = WalkOptions::default();
        let sw = RelationStepWalker::new(b(10), opts).test_unwrap();
        // Queue contains the start node, so !is_done().
        assert!(!sw.is_done());
    }

    #[test]
    fn test_hop_distance_same() {
        let d = hop_distance(&b(30), &b(30)).test_unwrap();
        assert_eq!(d, Some(0));
    }

    #[test]
    fn test_hop_distance_unreachable() {
        let d = hop_distance(&b(1), &b(200)).test_unwrap();
        assert!(d.is_none());
    }

    #[test]
    fn test_walk_options_shallow() {
        let opts = WalkOptions::shallow(3);
        assert_eq!(opts.max_depth, 3);
        assert_eq!(opts.max_nodes, 256);
    }

    #[test]
    fn test_walk_options_with_kind() {
        let opts = WalkOptions::with_kind(RelationKind::Parent);
        assert_eq!(opts.kind_filter, Some(RelationKind::Parent));
    }

    #[test]
    fn test_step_walker_finish() {
        let opts = WalkOptions::default();
        let sw = RelationStepWalker::new(b(55), opts).test_unwrap();
        let result = sw.finish();
        assert_eq!(result.n_visited(), 1);
    }

    #[test]
    fn test_nodes_at_depth_zero() {
        let res = nodes_at_depth(&b(60), 0).test_unwrap();
        // At depth 0, returns the start node.
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].as_bytes(), b(60).as_bytes());
    }

    #[test]
    fn test_reachable_from_all_isolated() {
        let res = reachable_from_all(&b(77)).test_unwrap();
        assert_eq!(res.len(), 1);
    }
}
