//! relation_walker.rs — Parcours itératif du graphe de relations ExoFS
//!
//! Règles appliquées :
//!  - RECUR-01 : zéro récursion — BFS/DFS à file/pile explicite
//!  - OOM-02   : try_reserve avant tout push
//!  - ARITH-02 : arithmétique vérifiée

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use super::relation_type::RelationKind;
use super::relation_graph::RELATION_GRAPH;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Profondeur maximale de parcours par défaut.
pub const WALKER_DEFAULT_MAX_DEPTH: u32 = 64;

/// Nombre maximum de nœuds visités par défaut.
pub const WALKER_DEFAULT_MAX_NODES: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// WalkResult
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un parcours de graphe.
#[derive(Clone, Debug, Default)]
pub struct WalkResult {
    /// Tous les blobs visités (ordre de découverte).
    pub visited:           Vec<BlobId>,
    /// Profondeur maximum atteinte.
    pub depth_reached:     u32,
    /// Nombre de transitions empruntées.
    pub n_edges_traversed: u32,
    /// `true` si le parcours a été interrompu (limite dépassée).
    pub truncated:         bool,
}

impl WalkResult {
    /// `true` si le nœud a été visité.
    pub fn contains(&self, blob: &BlobId) -> bool {
        self.visited.iter().any(|b| b.as_bytes() == blob.as_bytes())
    }

    /// Nombre de nœuds visités.
    pub fn n_visited(&self) -> usize { self.visited.len() }

    /// `true` si le résultat est vide.
    pub fn is_empty(&self) -> bool { self.visited.is_empty() }
}

// ─────────────────────────────────────────────────────────────────────────────
// WalkOptions
// ─────────────────────────────────────────────────────────────────────────────

/// Options configurables pour un parcours.
#[derive(Clone, Debug)]
pub struct WalkOptions {
    /// Profondeur maximale.
    pub max_depth: u32,
    /// Nombre maximum de nœuds à visiter.
    pub max_nodes: usize,
    /// Filtrer par kind (aucun filtre si `None`).
    pub kind_filter: Option<RelationKind>,
    /// Inclure les boucles (arc de A vers A).
    pub include_self_loops: bool,
}

impl Default for WalkOptions {
    fn default() -> Self {
        WalkOptions {
            max_depth:          WALKER_DEFAULT_MAX_DEPTH,
            max_nodes:          WALKER_DEFAULT_MAX_NODES,
            kind_filter:        None,
            include_self_loops: false,
        }
    }
}

impl WalkOptions {
    /// Options pour un parcours en largeur (par défaut).
    pub fn bfs_default() -> Self { Self::default() }

    /// Options restreintes (faible profondeur, peu de nœuds).
    pub fn shallow(depth: u32) -> Self {
        WalkOptions { max_depth: depth, max_nodes: 256, ..Default::default() }
    }

    /// Options avec filtre sur le type de relation.
    pub fn with_kind(kind: RelationKind) -> Self {
        WalkOptions { kind_filter: Some(kind), ..Default::default() }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationWalker
// ─────────────────────────────────────────────────────────────────────────────

/// Parcours itératif du graphe de relations.
///
/// Supporte BFS (breadth-first) et DFS (depth-first itératif).
/// RECUR-01 : aucune récursion.
pub struct RelationWalker {
    pub max_depth: u32,
    pub options: WalkOptions,
}

impl RelationWalker {
    /// Crée un walker avec la profondeur maximale donnée.
    pub fn new(max_depth: u32) -> Self {
        RelationWalker {
            max_depth,
            options: WalkOptions { max_depth, ..Default::default() },
        }
    }

    /// Crée un walker avec des options complètes.
    pub fn with_options(options: WalkOptions) -> Self {
        let max_depth = options.max_depth;
        RelationWalker { max_depth, options }
    }

    // ── BFS ──────────────────────────────────────────────────────────────────

    /// BFS depuis `start`.
    ///
    /// Itératif (RECUR-01) : utilise `VecDeque` comme file.
    pub fn bfs(&self, start: &BlobId) -> ExofsResult<WalkResult> {
        let mut visited: BTreeMap<[u8; 32], u32> = BTreeMap::new();
        let mut queue: VecDeque<(BlobId, u32)> = VecDeque::new();
        let mut result = WalkResult::default();

        visited.insert(*start.as_bytes(), 0);
        queue.push_back((*start, 0));

        while let Some((node, depth)) = queue.pop_front() {
            result.visited.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            result.visited.push(node);
            if depth > result.depth_reached { result.depth_reached = depth; }

            if result.visited.len() >= self.options.max_nodes {
                result.truncated = true;
                break;
            }

            let next_depth = depth
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;
            if next_depth > self.options.max_depth { continue; }

            let neighbors = self.collect_neighbors(&node);
            for nbr in neighbors {
                if !self.options.include_self_loops
                    && nbr.as_bytes() == node.as_bytes()
                {
                    continue;
                }
                if !visited.contains_key(nbr.as_bytes()) {
                    visited.insert(*nbr.as_bytes(), next_depth);
                    queue.push_back((nbr, next_depth));
                    result.n_edges_traversed = result.n_edges_traversed
                        .checked_add(1)
                        .ok_or(ExofsError::OffsetOverflow)?;
                }
            }
        }

        Ok(result)
    }

    // ── DFS ──────────────────────────────────────────────────────────────────

    /// DFS depuis `start`.
    ///
    /// Itératif (RECUR-01) : utilise `Vec` comme pile explicite.
    pub fn dfs(&self, start: &BlobId) -> ExofsResult<WalkResult> {
        let mut visited: BTreeMap<[u8; 32], ()> = BTreeMap::new();
        let mut stack: Vec<(BlobId, u32)> = Vec::new();
        let mut result = WalkResult::default();

        stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        stack.push((*start, 0));

        while let Some((node, depth)) = stack.pop() {
            let key = *node.as_bytes();
            if visited.contains_key(&key) { continue; }

            visited.insert(key, ());

            result.visited.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            result.visited.push(node);
            if depth > result.depth_reached { result.depth_reached = depth; }

            if result.visited.len() >= self.options.max_nodes {
                result.truncated = true;
                break;
            }

            let next_depth = depth
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;
            if next_depth > self.options.max_depth { continue; }

            let neighbors = self.collect_neighbors(&node);
            // Inversion pour conserver l'ordre naturel dans la pile.
            for nbr in neighbors.into_iter().rev() {
                if !visited.contains_key(nbr.as_bytes()) {
                    stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    stack.push((nbr, next_depth));
                    result.n_edges_traversed = result.n_edges_traversed
                        .checked_add(1)
                        .ok_or(ExofsError::OffsetOverflow)?;
                }
            }
        }

        Ok(result)
    }

    // ── Recherche de chemin ───────────────────────────────────────────────────

    /// BFS jusqu'à un nœud cible.  Retourne le chemin ou `None` si inaccessible.
    ///
    /// Le chemin retourné inclut `start` et `target`.
    pub fn find_path(
        &self,
        start:  &BlobId,
        target: &BlobId,
    ) -> ExofsResult<Option<Vec<BlobId>>> {
        // parent[node] = parent dans l'arbre BFS.
        let mut parent: BTreeMap<[u8; 32], [u8; 32]> = BTreeMap::new();
        let mut queue: VecDeque<(BlobId, u32)> = VecDeque::new();
        let sentinel = [0xFFu8; 32]; // racine sans parent

        parent.insert(*start.as_bytes(), sentinel);
        queue.push_back((*start, 0));

        while let Some((node, depth)) = queue.pop_front() {
            if node.as_bytes() == target.as_bytes() {
                // Reconstruction du chemin (RECUR-01 : boucle while)
                let mut path: Vec<BlobId> = Vec::new();
                let mut cur = *target.as_bytes();
                loop {
                    path.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    path.push(BlobId(cur));
                    let p = parent[&cur];
                    if p == sentinel { break; }
                    cur = p;
                }
                path.reverse();
                return Ok(Some(path));
            }

            let next_depth = depth
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;
            if next_depth > self.options.max_depth { continue; }

            for nbr in self.collect_neighbors(&node) {
                if !parent.contains_key(nbr.as_bytes()) {
                    parent.insert(*nbr.as_bytes(), *node.as_bytes());
                    queue.push_back((nbr, next_depth));
                }
            }
        }

        Ok(None)
    }

    /// `true` si `target` est accessible depuis `start`.
    pub fn is_reachable(&self, start: &BlobId, target: &BlobId) -> ExofsResult<bool> {
        // Optimisation: si start == target, c'est immédiatement vrai.
        if start.as_bytes() == target.as_bytes() { return Ok(true); }
        Ok(self.find_path(start, target)?.is_some())
    }

    // ── Helpers privés ───────────────────────────────────────────────────────

    fn collect_neighbors(&self, node: &BlobId) -> Vec<BlobId> {
        match self.options.kind_filter {
            None    => RELATION_GRAPH.get_neighbors(node),
            Some(k) => RELATION_GRAPH.get_neighbors_by_kind(node, k),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationStepWalker — parcours pas-à-pas
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un pas du walker pas-à-pas.
#[derive(Debug)]
pub enum StepResult {
    /// Un nouveau nœud a été découvert.
    Visited(BlobId),
    /// Parcours terminé (aucun nœud restant).
    Done,
    /// Erreur fatale.
    Error(ExofsError),
}

/// Walker pas-à-pas : chaque appel à `step()` découvre un nœud.
///
/// Utile pour intercaler la découverte avec d'autres traitements.
pub struct RelationStepWalker {
    options: WalkOptions,
    queue:   VecDeque<(BlobId, u32)>,
    visited: BTreeMap<[u8; 32], u32>,
    pub result: WalkResult,
}

impl RelationStepWalker {
    /// Crée un walker pas-à-pas depuis `start`.
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

    /// Avance d'un nœud dans le BFS.
    pub fn step(&mut self) -> StepResult {
        let (node, depth) = match self.queue.pop_front() {
            None    => return StepResult::Done,
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
            None    => return StepResult::Error(ExofsError::OffsetOverflow),
            Some(d) => d,
        };

        if next_depth <= self.options.max_depth {
            let neighbors = match self.options.kind_filter {
                None    => RELATION_GRAPH.get_neighbors(&node),
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

    /// Achève le parcours complet et retourne le résultat.
    pub fn finish(mut self) -> WalkResult {
        loop {
            match self.step() {
                StepResult::Done | StepResult::Error(_) => break,
                StepResult::Visited(_) => {}
            }
        }
        self.result
    }

    /// `true` si la file est vide (parcours terminé).
    pub fn is_done(&self) -> bool { self.queue.is_empty() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Tous les blobs accessibles depuis `start` sans limite de profondeur.
pub fn reachable_from_all(start: &BlobId) -> ExofsResult<Vec<BlobId>> {
    let w = RelationWalker::new(u32::MAX / 2);
    Ok(w.bfs(start)?.visited)
}

/// Distance en nombre de sauts entre deux blobs.
/// Retourne `None` si `target` est inaccessible.
pub fn hop_distance(start: &BlobId, target: &BlobId) -> ExofsResult<Option<u32>> {
    if start.as_bytes() == target.as_bytes() { return Ok(Some(0)); }
    let w = RelationWalker::new(WALKER_DEFAULT_MAX_DEPTH);
    match w.find_path(start, target)? {
        None       => Ok(None),
        Some(path) => {
            let n = path.len().saturating_sub(1) as u32;
            Ok(Some(n))
        }
    }
}

/// Collecte tous les blobs à exactement `depth` sauts de `start`.
pub fn nodes_at_depth(start: &BlobId, depth: u32) -> ExofsResult<Vec<BlobId>> {
    let mut current_layer: Vec<BlobId> = Vec::new();
    current_layer.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
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
                    next_layer.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
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
mod tests {
    use super::*;

    fn b(n: u8) -> BlobId { BlobId([n; 32]) }

    #[test] fn test_walk_result_contains() {
        let mut r = WalkResult::default();
        r.visited.push(b(5));
        assert!(r.contains(&b(5)));
        assert!(!r.contains(&b(6)));
    }

    #[test] fn test_walk_result_empty() {
        let r = WalkResult::default();
        assert!(r.is_empty());
        assert_eq!(r.n_visited(), 0);
    }

    #[test] fn test_bfs_isolated_node() {
        let w = RelationWalker::new(10);
        let res = w.bfs(&b(99)).unwrap();
        // Pas d'arcs depuis ce nœud → seul lui est visité.
        assert_eq!(res.n_visited(), 1);
        assert_eq!(res.depth_reached, 0);
        assert!(!res.truncated);
    }

    #[test] fn test_dfs_isolated_node() {
        let w = RelationWalker::new(5);
        let res = w.dfs(&b(70)).unwrap();
        assert_eq!(res.n_visited(), 1);
    }

    #[test] fn test_find_path_not_reachable() {
        let w = RelationWalker::new(5);
        let res = w.find_path(&b(50), &b(51)).unwrap();
        assert!(res.is_none());
    }

    #[test] fn test_find_path_self() {
        let w = RelationWalker::new(5);
        let res = w.find_path(&b(30), &b(30)).unwrap();
        assert!(res.is_some());
        let path = res.unwrap();
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].as_bytes(), b(30).as_bytes());
    }

    #[test] fn test_is_reachable_self() {
        let w = RelationWalker::new(5);
        assert!(w.is_reachable(&b(80), &b(80)).unwrap());
    }

    #[test] fn test_is_reachable_no_arc() {
        let w = RelationWalker::new(5);
        assert!(!w.is_reachable(&b(10), &b(11)).unwrap());
    }

    #[test] fn test_max_nodes_truncation() {
        let opts = WalkOptions { max_nodes: 1, ..Default::default() };
        let w = RelationWalker::with_options(opts);
        let res = w.bfs(&b(40)).unwrap();
        assert!(res.truncated);
        assert_eq!(res.n_visited(), 1);
    }

    #[test] fn test_step_walker_visits_start() {
        let opts = WalkOptions::default();
        let mut sw = RelationStepWalker::new(b(20), opts).unwrap();
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

    #[test] fn test_step_walker_is_done() {
        let opts = WalkOptions::default();
        let sw = RelationStepWalker::new(b(10), opts).unwrap();
        // La queue contient le nœud de départ donc !is_done().
        assert!(!sw.is_done());
    }

    #[test] fn test_hop_distance_same() {
        let d = hop_distance(&b(30), &b(30)).unwrap();
        assert_eq!(d, Some(0));
    }

    #[test] fn test_hop_distance_unreachable() {
        let d = hop_distance(&b(1), &b(200)).unwrap();
        assert!(d.is_none());
    }

    #[test] fn test_walk_options_shallow() {
        let opts = WalkOptions::shallow(3);
        assert_eq!(opts.max_depth, 3);
        assert_eq!(opts.max_nodes, 256);
    }

    #[test] fn test_walk_options_with_kind() {
        let opts = WalkOptions::with_kind(RelationKind::Parent);
        assert_eq!(opts.kind_filter, Some(RelationKind::Parent));
    }

    #[test] fn test_step_walker_finish() {
        let opts = WalkOptions::default();
        let sw = RelationStepWalker::new(b(55), opts).unwrap();
        let result = sw.finish();
        assert_eq!(result.n_visited(), 1);
    }

    #[test] fn test_nodes_at_depth_zero() {
        let res = nodes_at_depth(&b(60), 0).unwrap();
        // À profondeur 0, on retourne le nœud de départ.
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].as_bytes(), b(60).as_bytes());
    }

    #[test] fn test_reachable_from_all_isolated() {
        let res = reachable_from_all(&b(77)).unwrap();
        assert_eq!(res.len(), 1);
    }
}
