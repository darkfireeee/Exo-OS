//! relation_cycle.rs — Détection de cycles itérative dans le DAG ExoFS
//!
//! Règles appliquées :
//!  - RECUR-01 : zéro récursion — DFS à pile explicite obligatoirement
//!  - OOM-02   : try_reserve systématique
//!  - ARITH-02 : arithmétique vérifiée


extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use super::relation_graph::RELATION_GRAPH;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Profondeur maximale de détection par défaut.
pub const CYCLE_DEFAULT_MAX_DEPTH: u32 = 128;

/// Nombre maximum de nœuds visités lors d'une détection.
pub const CYCLE_DEFAULT_MAX_NODES: usize = 8192;

// ─────────────────────────────────────────────────────────────────────────────
// VisitState — états DFS
// ─────────────────────────────────────────────────────────────────────────────

/// État de visite d'un nœud dans le DFS de détection de cycles.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum VisitState {
    /// Non encore visité.
    Unvisited = 0,
    /// Dans la pile DFS courante (potentiel cycle).
    InStack   = 1,
    /// Traitement terminé (aucun cycle depuis ce nœud).
    Done      = 2,
}

// ─────────────────────────────────────────────────────────────────────────────
// CycleReport
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une détection de cycle.
#[derive(Clone, Debug, Default)]
pub struct CycleReport {
    /// `true` si un cycle a été détecté.
    pub has_cycle:  bool,
    /// Nœuds du cycle (vide si pas de cycle).
    pub cycle_path: Vec<BlobId>,
    /// Nombre de nœuds explorés avant de conclure.
    pub n_explored: u32,
}

impl CycleReport {
    /// Longueur du cycle détecté (0 si pas de cycle).
    pub fn cycle_len(&self) -> usize { self.cycle_path.len() }

    /// Rapport sans cycle.
    pub fn no_cycle(n_explored: u32) -> Self {
        CycleReport { has_cycle: false, cycle_path: Vec::new(), n_explored }
    }

    /// Rapport avec cycle.
    pub fn with_cycle(path: Vec<BlobId>, n_explored: u32) -> Self {
        CycleReport { has_cycle: true, cycle_path: path, n_explored }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DFS itératif — état de frame
// ─────────────────────────────────────────────────────────────────────────────

/// Un cadre de la pile DFS itérative.
struct DfsFrame {
    node:           BlobId,
    neighbor_idx:   usize,
    neighbors:      Vec<BlobId>,
    depth:          u32,
}

impl DfsFrame {
    fn new(node: BlobId, depth: u32) -> ExofsResult<Self> {
        let neighbors = RELATION_GRAPH.get_neighbors(&node);
        Ok(DfsFrame { node, neighbor_idx: 0, neighbors, depth })
    }

    /// Prochain voisin non encore traité, ou `None` si terminé.
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

/// Détecteur de cycles dans le graphe de relations.
///
/// Utilise un DFS itératif à coloration tri-état (Unvisited / InStack / Done).
/// RECUR-01 : aucun appel récursif.
pub struct RelationCycleDetector;

impl RelationCycleDetector {
    /// Détecte si un cycle est accessible depuis `start`.
    ///
    /// Itératif (RECUR-01) : pile explicite `Vec<DfsFrame>`.
    pub fn detect_from(
        start:     BlobId,
        max_depth: u32,
    ) -> ExofsResult<CycleReport> {
        let mut state:  BTreeMap<[u8; 32], VisitState> = BTreeMap::new();
        let mut path:   Vec<BlobId>   = Vec::new(); // chemin courant
        let mut stack:  Vec<DfsFrame> = Vec::new();
        let mut n_explored: u32 = 0;

        // Initialisation
        state.insert(*start.as_bytes(), VisitState::InStack);
        path.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        path.push(start);
        stack.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        stack.push(DfsFrame::new(start, 0)?);

        // Boucle principale (RECUR-01)
        while let Some(frame) = stack.last_mut() {
            n_explored = n_explored.checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;

            if n_explored as usize > CYCLE_DEFAULT_MAX_NODES {
                // Limite de sécurité — pas de cycle trouvé dans la fenêtre.
                return Ok(CycleReport::no_cycle(n_explored));
            }

            // Chercher le prochain voisin non traité.
            let next = frame.next_neighbor();
            let cur_depth = frame.depth;

            match next {
                None => {
                    // Tous les voisins ont été traités : ce nœud est Done.
                    let node = frame.node;
                    stack.pop();
                    path.pop();
                    state.insert(*node.as_bytes(), VisitState::Done);
                }
                Some(nbr) => {
                    if cur_depth >= max_depth { continue; }

                    let nbr_key = *nbr.as_bytes();
                    let s = state.get(&nbr_key).copied().unwrap_or(VisitState::Unvisited);

                    match s {
                        VisitState::InStack => {
                            // Cycle détecté !
                            // Reconstituer le segment du cycle depuis `path`.
                            let mut cycle: Vec<BlobId> = Vec::new();
                            // Trouver l'index du nœud dans path.
                            let start_idx = path
                                .iter()
                                .position(|b| b.as_bytes() == &nbr_key)
                                .unwrap_or(0);
                            cycle.try_reserve(path.len() - start_idx + 1)
                                .map_err(|_| ExofsError::NoMemory)?;
                            for n in &path[start_idx..] {
                                cycle.push(*n);
                            }
                            cycle.push(nbr); // arête de retour
                            return Ok(CycleReport::with_cycle(cycle, n_explored));
                        }
                        VisitState::Done => {
                            // Déjà entièrement traité, pas de cycle par ici.
                        }
                        VisitState::Unvisited => {
                            let new_depth = cur_depth
                                .checked_add(1)
                                .ok_or(ExofsError::OffsetOverflow)?;
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

    /// Lance la détection depuis tous les nœuds du graphe.
    ///
    /// Retourne le premier cycle trouvé ou un rapport sans cycle.
    pub fn detect_global(max_depth: u32) -> ExofsResult<CycleReport> {
        let all_edges = RELATION_GRAPH.all_edges();
        let mut seen: BTreeMap<[u8; 32], ()> = BTreeMap::new();

        for (from, _) in all_edges {
            if seen.contains_key(from.as_bytes()) { continue; }
            seen.insert(*from.as_bytes(), ());
            let report = Self::detect_from(from, max_depth)?;
            if report.has_cycle { return Ok(report); }
        }

        Ok(CycleReport::no_cycle(seen.len() as u32))
    }

    /// Vérifie qu'ajouter l'arc `from → to` ne créerait pas de cycle.
    ///
    /// Retourne `true` si l'ajout est sûr (pas de cycle).
    pub fn is_safe_to_add(from: &BlobId, to: &BlobId, max_depth: u32) -> ExofsResult<bool> {
        // Un cycle serait créé si `from` est déjà accessible depuis `to`.
        // On lance un BFS depuis `to` et on vérifie si `from` est atteignable.
        use alloc::collections::VecDeque;
        let mut visited: BTreeMap<[u8; 32], ()> = BTreeMap::new();
        let mut queue: VecDeque<(BlobId, u32)> = VecDeque::new();

        visited.insert(*to.as_bytes(), ());
        queue.push_back((*to, 0));

        // Si from == to, cycle immédiat.
        if from.as_bytes() == to.as_bytes() { return Ok(false); }

        while let Some((node, depth)) = queue.pop_front() {
            if node.as_bytes() == from.as_bytes() {
                return Ok(false); // cycle détecté si on ajoutait l'arc
            }
            let next_depth = depth
                .checked_add(1)
                .ok_or(ExofsError::OffsetOverflow)?;
            if next_depth > max_depth { continue; }

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
// TopologicalSorter — tri topologique itératif (Kahn)
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat du tri topologique.
#[derive(Debug, Default)]
pub struct TopoResult {
    /// Ordre topologique (du source vers les sinks).
    pub order:      Vec<BlobId>,
    /// `true` si un cycle a empêché un tri complet.
    pub has_cycle:  bool,
}

/// Tri topologique de Kahn (itératif, RECUR-01).
///
/// Requiert l'ensemble des nœuds et arcs fourni explicitement pour
/// éviter toute dépendance à un état global mutable.
pub struct TopologicalSorter;

impl TopologicalSorter {
    /// Tri topologique depuis les nœuds donnés.
    ///
    /// - `nodes`     : liste des nœuds à trier.
    /// - `edges`     : liste des arcs (from, to).
    pub fn sort(
        nodes: &[BlobId],
        edges: &[(BlobId, BlobId)],
    ) -> ExofsResult<TopoResult> {
        // Calcule in-degree pour chaque nœud.
        let mut in_degree: BTreeMap<[u8; 32], u32> = BTreeMap::new();
        let mut adj:       BTreeMap<[u8; 32], Vec<BlobId>> = BTreeMap::new();

        for n in nodes {
            in_degree.entry(*n.as_bytes()).or_insert(0);
            adj.entry(*n.as_bytes()).or_insert_with(Vec::new);
        }

        for (from, to) in edges {
            if let Some(v) = adj.get_mut(from.as_bytes()) {
                v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                v.push(*to);
            }
            *in_degree.entry(*to.as_bytes()).or_insert(0) =
                in_degree.get(to.as_bytes()).copied().unwrap_or(0)
                    .checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        }

        // File de départ : tous les nœuds à in-degree 0.
        let mut queue: alloc::collections::VecDeque<BlobId> =
            alloc::collections::VecDeque::new();
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
                    if *d == 0 { queue.push_back(nbr); }
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
mod tests {
    use super::*;

    fn b(n: u8) -> BlobId { BlobId([n; 32]) }

    // ── CycleDetector ────────────────────────────────────────────────────────

    #[test] fn test_detect_isolated_no_cycle() {
        // Nœud sans arcs sortants → pas de cycle.
        let r = RelationCycleDetector::detect_from(b(99), 10).unwrap();
        assert!(!r.has_cycle);
    }

    #[test] fn test_cycle_report_no_cycle() {
        let r = CycleReport::no_cycle(5);
        assert!(!r.has_cycle);
        assert_eq!(r.cycle_len(), 0);
        assert_eq!(r.n_explored, 5);
    }

    #[test] fn test_cycle_report_with_cycle() {
        let path = alloc::vec![b(1), b(2), b(3), b(1)];
        let r = CycleReport::with_cycle(path, 3);
        assert!(r.has_cycle);
        assert_eq!(r.cycle_len(), 4);
    }

    #[test] fn test_is_safe_to_add_self_loop() {
        // from == to → cycle immédiat.
        let safe = RelationCycleDetector::is_safe_to_add(&b(10), &b(10), 5).unwrap();
        assert!(!safe);
    }

    #[test] fn test_is_safe_no_existing_arc() {
        // Pas d'arc entre 20 et 21 → sûr d'ajouter.
        let safe = RelationCycleDetector::is_safe_to_add(&b(20), &b(21), 5).unwrap();
        assert!(safe);
    }

    #[test] fn test_detect_global_empty() {
        // Graphe vide (ou nœuds sans arcs globaux vers des blobs fictifs).
        let r = RelationCycleDetector::detect_global(10).unwrap();
        // Pas de cycle dans un graphe vide.
        assert!(!r.has_cycle);
    }

    // ── TopologicalSorter ────────────────────────────────────────────────────

    #[test] fn test_topo_sort_chain() {
        // 1 → 2 → 3
        let nodes = alloc::vec![b(1), b(2), b(3)];
        let edges = alloc::vec![(b(1), b(2)), (b(2), b(3))];
        let res = TopologicalSorter::sort(&nodes, &edges).unwrap();
        assert!(!res.has_cycle);
        assert_eq!(res.order.len(), 3);
        // 1 doit précéder 2 et 3.
        let pos1 = res.order.iter().position(|b| *b.as_bytes() == [1u8; 32]).unwrap();
        let pos2 = res.order.iter().position(|b| *b.as_bytes() == [2u8; 32]).unwrap();
        let pos3 = res.order.iter().position(|b| *b.as_bytes() == [3u8; 32]).unwrap();
        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
    }

    #[test] fn test_topo_sort_cycle() {
        // 1 → 2 → 1 (cycle)
        let nodes = alloc::vec![b(1), b(2)];
        let edges = alloc::vec![(b(1), b(2)), (b(2), b(1))];
        let res = TopologicalSorter::sort(&nodes, &edges).unwrap();
        assert!(res.has_cycle);
    }

    #[test] fn test_topo_sort_single_node() {
        let nodes = alloc::vec![b(5)];
        let edges: Vec<(BlobId, BlobId)> = alloc::vec![];
        let res = TopologicalSorter::sort(&nodes, &edges).unwrap();
        assert!(!res.has_cycle);
        assert_eq!(res.order.len(), 1);
    }

    #[test] fn test_topo_sort_diamond() {
        // 1 → 2, 1 → 3, 2 → 4, 3 → 4
        let nodes = alloc::vec![b(1), b(2), b(3), b(4)];
        let edges = alloc::vec![
            (b(1), b(2)), (b(1), b(3)), (b(2), b(4)), (b(3), b(4))
        ];
        let res = TopologicalSorter::sort(&nodes, &edges).unwrap();
        assert!(!res.has_cycle);
        assert_eq!(res.order.len(), 4);
    }
}
