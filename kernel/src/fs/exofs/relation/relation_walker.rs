//! RelationWalker — parcours BFS/DFS du graphe de relations ExoFS (no_std).

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::relation_type::RelationKind;
use super::relation_graph::RELATION_GRAPH;

/// Résultat d'un parcours.
#[derive(Clone, Debug)]
pub struct WalkResult {
    pub visited: Vec<BlobId>,
    pub depth:   Vec<u32>,   // Profondeur correspondante.
    pub n_edges: u64,
}

pub struct RelationWalker {
    max_depth: u32,
    kind_filter: Option<RelationKind>,
}

impl RelationWalker {
    pub fn new(max_depth: u32) -> Self {
        Self { max_depth, kind_filter: None }
    }

    pub fn with_kind_filter(mut self, kind: RelationKind) -> Self {
        self.kind_filter = Some(kind);
        self
    }

    /// Parcours en largeur (BFS) depuis `start`.
    pub fn bfs(&self, start: BlobId) -> Result<WalkResult, FsError> {
        let mut visited_map: BTreeMap<[u8; 32], u32> = BTreeMap::new();
        let mut queue: VecDeque<(BlobId, u32)> = VecDeque::new();
        let mut out_nodes = Vec::new();
        let mut out_depth = Vec::new();
        let mut n_edges = 0u64;

        queue.push_back((start, 0));
        visited_map.try_reserve(64).map_err(|_| FsError::OutOfMemory)?;
        visited_map.insert(start.as_bytes(), 0);

        while let Some((node, depth)) = queue.pop_front() {
            out_nodes.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            out_nodes.push(node);
            out_depth.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            out_depth.push(depth);

            if depth >= self.max_depth { continue; }

            let neighbors = match self.kind_filter {
                None      => RELATION_GRAPH.get_neighbors(&node),
                Some(k)   => RELATION_GRAPH.get_neighbors_by_kind(&node, k),
            };

            for nbr in neighbors {
                n_edges += 1;
                if !visited_map.contains_key(&nbr.as_bytes()) {
                    visited_map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                    visited_map.insert(nbr.as_bytes(), depth + 1);
                    queue.push_back((nbr, depth + 1));
                }
            }
        }

        Ok(WalkResult { visited: out_nodes, depth: out_depth, n_edges })
    }

    /// Parcours en profondeur (DFS) depuis `start`.
    pub fn dfs(&self, start: BlobId) -> Result<WalkResult, FsError> {
        let mut visited_map: BTreeMap<[u8; 32], u32> = BTreeMap::new();
        let mut stack: Vec<(BlobId, u32)> = Vec::new();
        let mut out_nodes = Vec::new();
        let mut out_depth = Vec::new();
        let mut n_edges = 0u64;

        stack.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        stack.push((start, 0));
        visited_map.try_reserve(64).map_err(|_| FsError::OutOfMemory)?;
        visited_map.insert(start.as_bytes(), 0);

        while let Some((node, depth)) = stack.pop() {
            out_nodes.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            out_nodes.push(node);
            out_depth.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            out_depth.push(depth);

            if depth >= self.max_depth { continue; }

            let neighbors = match self.kind_filter {
                None    => RELATION_GRAPH.get_neighbors(&node),
                Some(k) => RELATION_GRAPH.get_neighbors_by_kind(&node, k),
            };

            for nbr in neighbors {
                n_edges += 1;
                if !visited_map.contains_key(&nbr.as_bytes()) {
                    visited_map.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                    visited_map.insert(nbr.as_bytes(), depth + 1);
                    stack.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
                    stack.push((nbr, depth + 1));
                }
            }
        }

        Ok(WalkResult { visited: out_nodes, depth: out_depth, n_edges })
    }
}
