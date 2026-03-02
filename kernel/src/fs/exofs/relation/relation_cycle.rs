//! RelationCycleDetector — détection de cycles dans le DAG ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, FsError};
use super::relation_graph::RELATION_GRAPH;

/// Rapport de cycle détecté.
#[derive(Clone, Debug)]
pub struct CycleReport {
    pub has_cycle: bool,
    pub cycle_path: Vec<BlobId>,   // Noeuds constituant le cycle (si détecté).
}

/// États pour le DFS de détection de cycles.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Unvisited = 0,
    InStack   = 1,
    Done      = 2,
}

pub struct RelationCycleDetector;

impl RelationCycleDetector {
    /// Vérifie si un cycle est accessible depuis `start` (DFS coloration).
    pub fn detect_from(start: BlobId, max_depth: u32) -> Result<CycleReport, FsError> {
        let mut state: BTreeMap<[u8; 32], VisitState> = BTreeMap::new();
        let mut stack: Vec<BlobId> = Vec::new();
        let mut cycle_path: Vec<BlobId> = Vec::new();

        let found = Self::dfs(start, &mut state, &mut stack, &mut cycle_path, max_depth, 0)?;
        Ok(CycleReport { has_cycle: found, cycle_path })
    }

    fn dfs(
        node:       BlobId,
        state:      &mut BTreeMap<[u8; 32], VisitState>,
        path:       &mut Vec<BlobId>,
        cycle_path: &mut Vec<BlobId>,
        max_depth:  u32,
        depth:      u32,
    ) -> Result<bool, FsError> {
        if depth > max_depth { return Ok(false); }

        let key = node.as_bytes();
        match state.get(&key).copied().unwrap_or(VisitState::Unvisited) {
            VisitState::InStack => {
                // Cycle détecté : copier le chemin.
                cycle_path.try_reserve(path.len() + 1).map_err(|_| FsError::OutOfMemory)?;
                for n in path.iter() { cycle_path.push(*n); }
                cycle_path.push(node);
                return Ok(true);
            }
            VisitState::Done => return Ok(false),
            VisitState::Unvisited => {}
        }

        state.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        state.insert(key, VisitState::InStack);
        path.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        path.push(node);

        let neighbors = RELATION_GRAPH.get_neighbors(&node);
        for nbr in neighbors {
            if Self::dfs(nbr, state, path, cycle_path, max_depth, depth + 1)? {
                return Ok(true);
            }
        }

        path.pop();
        state.insert(key, VisitState::Done);
        Ok(false)
    }
}
