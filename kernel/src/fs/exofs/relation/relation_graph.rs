//! relation_graph.rs — Graphe d adjacence des relations ExoFS
//!
//! Règles appliquées :
//!  - RECUR-01 : aucune récursion
//!  - OOM-02   : try_reserve systématique
//!  - ARITH-02 : arithmétique vérifiée

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use crate::scheduler::sync::spinlock::SpinLock;
use super::relation::{Relation, RelationId};
use super::relation_type::{RelationKind, RelationWeight};

// ─────────────────────────────────────────────────────────────────────────────
// Edge — arête du graphe
// ─────────────────────────────────────────────────────────────────────────────

/// Arête dans le graphe d adjacence.
#[derive(Clone, Debug)]
pub struct Edge {
    pub id:     RelationId,
    pub to:     BlobId,
    pub kind:   RelationKind,
    pub weight: u32,
}

impl Edge {
    pub fn is_strong(&self) -> bool {
        RelationWeight(self.weight).is_strong()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GraphStats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct GraphStats {
    pub n_nodes:       usize,
    pub n_edges:       u64,
    pub total_adds:    u64,
    pub total_removes: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// GraphInner
// ─────────────────────────────────────────────────────────────────────────────

struct GraphInner {
    /// adj[from_key] = Vec<Edge>
    adj: BTreeMap<[u8; 32], Vec<Edge>>,
}

impl GraphInner {
    const fn new_empty() -> Self {
        GraphInner { adj: BTreeMap::new() }
    }

    fn add(&mut self, rel: &Relation) -> ExofsResult<()> {
        let from_key = *rel.from.as_bytes();
        let edge = Edge {
            id:     rel.id,
            to:     rel.to,
            kind:   rel.rel_type.kind,
            weight: rel.rel_type.weight_u32(),
        };
        if let Some(v) = self.adj.get_mut(&from_key) {
            // Pas de doublon.
            if v.iter().any(|e| e.id == rel.id) { return Ok(()); }
            v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            v.push(edge);
        } else {
            let mut v = Vec::new();
            v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            v.push(edge);
            self.adj.insert(from_key, v);
        }
        Ok(())
    }

    fn remove(&mut self, rel: &Relation) -> bool {
        let from_key = *rel.from.as_bytes();
        if let Some(v) = self.adj.get_mut(&from_key) {
            let before = v.len();
            v.retain(|e| e.id != rel.id);
            let removed = v.len() < before;
            if v.is_empty() { self.adj.remove(&from_key); }
            removed
        } else {
            false
        }
    }

    fn neighbors(&self, from: &[u8; 32]) -> Vec<BlobId> {
        self.adj.get(from)
            .map(|v| v.iter().map(|e| e.to).collect())
            .unwrap_or_default()
    }

    fn neighbors_by_kind(&self, from: &[u8; 32], kind: RelationKind) -> Vec<BlobId> {
        self.adj.get(from)
            .map(|v| v.iter()
                .filter(|e| e.kind == kind)
                .map(|e| e.to)
                .collect())
            .unwrap_or_default()
    }

    fn edges_from(&self, from: &[u8; 32]) -> Vec<Edge> {
        self.adj.get(from).cloned().unwrap_or_default()
    }

    fn strong_edges_from(&self, from: &[u8; 32]) -> Vec<Edge> {
        self.adj.get(from)
            .map(|v| v.iter().filter(|e| e.is_strong()).cloned().collect())
            .unwrap_or_default()
    }

    fn out_degree(&self, from: &[u8; 32]) -> usize {
        self.adj.get(from).map(|v| v.len()).unwrap_or(0)
    }

    fn n_edges_total(&self) -> u64 {
        self.adj.values().fold(0u64, |acc, v| {
            acc.saturating_add(v.len() as u64)
        })
    }

    fn flush(&mut self) { self.adj.clear(); }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationGraph (publique, thread-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// Graphe d adjacence des relations ExoFS, thread-safe.
pub struct RelationGraph {
    inner:      SpinLock<GraphInner>,
    total_adds: AtomicU64,
    total_rems: AtomicU64,
}

impl RelationGraph {
    pub const fn new_const() -> Self {
        RelationGraph {
            inner:      SpinLock::new(GraphInner::new_empty()),
            total_adds: AtomicU64::new(0),
            total_rems: AtomicU64::new(0),
        }
    }

    /// Ajoute une relation au graphe.
    pub fn add_relation(&self, rel: &Relation) -> ExofsResult<()> {
        self.inner.lock().add(rel)?;
        self.total_adds.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Supprime une relation du graphe.
    pub fn remove_relation(&self, rel: &Relation) -> bool {
        let removed = self.inner.lock().remove(rel);
        if removed { self.total_rems.fetch_add(1, Ordering::Relaxed); }
        removed
    }

    /// Tous les voisins directs de `from`.
    pub fn get_neighbors(&self, from: &BlobId) -> Vec<BlobId> {
        self.inner.lock().neighbors(from.as_bytes())
    }

    /// Voisins d un certain type.
    pub fn get_neighbors_by_kind(&self, from: &BlobId, kind: RelationKind) -> Vec<BlobId> {
        self.inner.lock().neighbors_by_kind(from.as_bytes(), kind)
    }

    /// Toutes les arêtes sortantes de `from`.
    pub fn get_edges_from(&self, from: &BlobId) -> Vec<Edge> {
        self.inner.lock().edges_from(from.as_bytes())
    }

    /// Arêtes sortantes fortes (weight ≥ STRONG).
    pub fn get_strong_edges(&self, from: &BlobId) -> Vec<Edge> {
        self.inner.lock().strong_edges_from(from.as_bytes())
    }

    /// Degré sortant d un nœud.
    pub fn out_degree(&self, from: &BlobId) -> usize {
        self.inner.lock().out_degree(from.as_bytes())
    }

    /// Nombre de nœuds (blobs avec au moins une relation sortante).
    pub fn n_nodes(&self) -> usize { self.inner.lock().adj.len() }

    /// Nombre total d arêtes.
    pub fn n_edges(&self) -> u64 { self.inner.lock().n_edges_total() }

    /// Statistiques.
    pub fn stats(&self) -> GraphStats {
        GraphStats {
            n_nodes:       self.n_nodes(),
            n_edges:       self.n_edges(),
            total_adds:    self.total_adds.load(Ordering::Relaxed),
            total_removes: self.total_rems.load(Ordering::Relaxed),
        }
    }

    /// `true` si un arc direct `from → to` (de n importe quel type) existe.
    pub fn has_direct_edge(&self, from: &BlobId, to: &BlobId) -> bool {
        let guard = self.inner.lock();
        guard.adj.get(from.as_bytes())
            .map(|v| v.iter().any(|e| e.to.as_bytes() == to.as_bytes()))
            .unwrap_or(false)
    }

    /// `true` si un arc direct d un type précis existe.
    pub fn has_typed_edge(&self, from: &BlobId, to: &BlobId, kind: RelationKind) -> bool {
        let guard = self.inner.lock();
        guard.adj.get(from.as_bytes())
            .map(|v| v.iter().any(|e| {
                e.kind == kind && e.to.as_bytes() == to.as_bytes()
            }))
            .unwrap_or(false)
    }

    /// Vide le graphe.
    pub fn flush(&self) {
        self.inner.lock().flush();
        self.total_adds.store(0, Ordering::Relaxed);
        self.total_rems.store(0, Ordering::Relaxed);
    }

    /// Copie toutes les arêtes de tous les nœuds (pour dump/inspection).
    pub fn all_edges(&self) -> Vec<(BlobId, Edge)> {
        let guard = self.inner.lock();
        let mut out = Vec::new();
        for (key, edges) in guard.adj.iter() {
            let from = BlobId(*key);
            for e in edges {
                let _ = out.try_reserve(1)
                    .map(|_| out.push((from, e.clone())));
            }
        }
        out
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────────────────────

pub static RELATION_GRAPH: RelationGraph = RelationGraph::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::relation_type::RelationType;

    fn blob(b: u8) -> BlobId { BlobId([b; 32]) }

    fn rel(id: u64, from: BlobId, to: BlobId) -> Relation {
        Relation::new(
            RelationId(id), from, to,
            RelationType::new(RelationKind::Parent), 0,
        )
    }

    #[test] fn test_add_neighbors() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(1, blob(1), blob(2))).unwrap();
        let n = g.get_neighbors(&blob(1));
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].as_bytes(), &[2u8; 32]);
    }

    #[test] fn test_remove_edge() {
        let g = RelationGraph::new_const();
        let r = rel(2, blob(3), blob(4));
        g.add_relation(&r).unwrap();
        g.remove_relation(&r);
        assert!(g.get_neighbors(&blob(3)).is_empty());
    }

    #[test] fn test_out_degree() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(3, blob(5), blob(6))).unwrap();
        g.add_relation(&rel(4, blob(5), blob(7))).unwrap();
        assert_eq!(g.out_degree(&blob(5)), 2);
    }

    #[test] fn test_has_direct_edge() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(5, blob(10), blob(11))).unwrap();
        assert!(g.has_direct_edge(&blob(10), &blob(11)));
        assert!(!g.has_direct_edge(&blob(11), &blob(10)));
    }

    #[test] fn test_no_duplicate() {
        let g = RelationGraph::new_const();
        let r = rel(6, blob(20), blob(21));
        g.add_relation(&r).unwrap();
        g.add_relation(&r).unwrap();
        assert_eq!(g.out_degree(&blob(20)), 1);
    }

    #[test] fn test_n_edges() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(7, blob(30), blob(31))).unwrap();
        g.add_relation(&rel(8, blob(30), blob(32))).unwrap();
        assert_eq!(g.n_edges(), 2);
    }

    #[test] fn test_stats() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(9, blob(40), blob(41))).unwrap();
        let s = g.stats();
        assert_eq!(s.n_nodes, 1);
        assert_eq!(s.total_adds, 1);
    }

    #[test] fn test_n_nodes_increments() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(10, blob(50), blob(51))).unwrap();
        g.add_relation(&rel(11, blob(52), blob(53))).unwrap();
        // 2 sources distinctes → 2 nœuds.
        assert_eq!(g.n_nodes(), 2);
    }

    #[test] fn test_get_neighbors_by_kind() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(12, blob(60), blob(61))).unwrap();
        let nbrs = g.get_neighbors_by_kind(&blob(60), RelationKind::Parent);
        assert_eq!(nbrs.len(), 1);
        let nbrs_clone = g.get_neighbors_by_kind(&blob(60), RelationKind::Clone);
        assert!(nbrs_clone.is_empty());
    }

    #[test] fn test_get_edges_from() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(13, blob(70), blob(71))).unwrap();
        g.add_relation(&rel(14, blob(70), blob(72))).unwrap();
        let edges = g.get_edges_from(&blob(70));
        assert_eq!(edges.len(), 2);
    }

    #[test] fn test_all_edges() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(15, blob(80), blob(81))).unwrap();
        let all = g.all_edges();
        assert!(!all.is_empty());
    }

    #[test] fn test_has_typed_edge() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(16, blob(90), blob(91))).unwrap();
        assert!(g.has_typed_edge(&blob(90), &blob(91), RelationKind::Parent));
        assert!(!g.has_typed_edge(&blob(90), &blob(91), RelationKind::Clone));
    }

    #[test] fn test_flush_resets() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(17, blob(100), blob(101))).unwrap();
        g.flush();
        assert_eq!(g.n_nodes(), 0);
        assert_eq!(g.n_edges(), 0);
    }

    #[test] fn test_get_strong_edges_empty() {
        let g = RelationGraph::new_const();
        g.add_relation(&rel(18, blob(110), blob(111))).unwrap();
        // rel() crée des relations avec weight 0 → pas strong
        let strong = g.get_strong_edges(&blob(110));
        assert!(strong.is_empty());
    }

    #[test] fn test_stats_removes() {
        let g = RelationGraph::new_const();
        let r = rel(19, blob(120), blob(121));
        g.add_relation(&r).unwrap();
        g.remove_relation(&r);
        let s = g.stats();
        assert!(s.total_removes >= 1);
    }

    #[test] fn test_has_no_edge_on_empty() {
        let g = RelationGraph::new_const();
        assert!(!g.has_direct_edge(&blob(200), &blob(201)));
        assert_eq!(g.out_degree(&blob(200)), 0);
        assert!(g.get_neighbors(&blob(200)).is_empty());
    }
}
