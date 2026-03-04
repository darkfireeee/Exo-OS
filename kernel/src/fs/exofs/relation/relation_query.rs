//! relation_query.rs — Requêtes haut-niveau sur le graphe ExoFS
//!
//! Règles appliquées :
//!  - RECUR-01 : aucune récursion
//!  - OOM-02   : try_reserve avant tout push

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use super::relation::{Relation, RelationId};
use super::relation_type::{RelationKind, RelationFilter, RelationDirection};
use super::relation_storage::RELATION_STORAGE;
use super::relation_index::RELATION_INDEX;
use super::relation_graph::RELATION_GRAPH;

// ─────────────────────────────────────────────────────────────────────────────
// QueryResult
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat enrichi d une requête de relations.
#[derive(Clone, Debug, Default)]
pub struct QueryResult {
    pub relations: Vec<Relation>,
    pub n_total:   usize,
    pub truncated: bool,
}

impl QueryResult {
    fn from_rels(rels: Vec<Relation>) -> Self {
        let n = rels.len();
        QueryResult { relations: rels, n_total: n, truncated: false }
    }

    fn empty() -> Self { Self::default() }

    /// Filtre les relations du résultat selon un prédicat.
    pub fn filter_by<F>(mut self, pred: F) -> Self
    where
        F: Fn(&Relation) -> bool,
    {
        self.relations.retain(|r| pred(r));
        self.n_total = self.relations.len();
        self
    }

    /// Limite le nombre de résultats.
    pub fn limit(mut self, max: usize) -> Self {
        if self.relations.len() > max {
            self.relations.truncate(max);
            self.truncated = true;
        }
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationQuery
// ─────────────────────────────────────────────────────────────────────────────

/// Façade de requêtes sur le graphe de relations.
pub struct RelationQuery;

impl RelationQuery {
    /// Toutes les relations sortantes d un blob.
    pub fn outgoing(from: &BlobId) -> ExofsResult<QueryResult> {
        let ids = RELATION_INDEX.ids_from(from);
        Self::load_ids(&ids)
    }

    /// Toutes les relations entrantes vers un blob.
    pub fn incoming(to: &BlobId) -> ExofsResult<QueryResult> {
        let ids = RELATION_INDEX.ids_to(to);
        Self::load_ids(&ids)
    }

    /// Relations dans la direction donnée.
    pub fn in_direction(
        blob:      &BlobId,
        direction: RelationDirection,
    ) -> ExofsResult<QueryResult> {
        let ids = RELATION_INDEX.ids_in_direction(blob, direction);
        Self::load_ids(&ids)
    }

    /// Relations sortantes d un certain type.
    pub fn outgoing_by_kind(
        from: &BlobId,
        kind: RelationKind,
    ) -> ExofsResult<QueryResult> {
        let ids = RELATION_INDEX.ids_from(from);
        let mut rels: Vec<Relation> = Vec::new();
        for id in ids {
            if let Some(r) = RELATION_STORAGE.load(id) {
                let r = r?;
                if r.rel_type.kind == kind {
                    rels.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    rels.push(r);
                }
            }
        }
        Ok(QueryResult::from_rels(rels))
    }

    /// Relations avec un filtre composite.
    pub fn filtered(blob: &BlobId, filter: RelationFilter) -> ExofsResult<QueryResult> {
        let direction = filter.direction.unwrap_or(RelationDirection::Outgoing);
        let ids = RELATION_INDEX.ids_in_direction(blob, direction);
        let mut rels: Vec<Relation> = Vec::new();
        for id in ids {
            if let Some(r) = RELATION_STORAGE.load(id) {
                let r = r?;
                if filter.matches(r.rel_type) {
                    rels.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    rels.push(r);
                }
            }
        }
        Ok(QueryResult::from_rels(rels))
    }

    /// Vérifie l existence d une relation directe `from → to` d un certain type.
    pub fn has_direct_relation(
        from: &BlobId,
        to:   &BlobId,
        kind: RelationKind,
    ) -> bool {
        RELATION_GRAPH.has_typed_edge(from, to, kind)
    }

    /// Cherche tous les blobs accessibles depuis `start` en BFS.
    /// (RECUR-01 — implémenté dans RelationWalker)
    pub fn reachable_from(start: &BlobId, max_depth: u32) -> ExofsResult<Vec<BlobId>> {
        use super::relation_walker::RelationWalker;
        let w = RelationWalker::new(max_depth);
        Ok(w.bfs(start)?.visited)
    }

    /// Cherche les blobs qui peuvent atteindre `target` en remontant les arcs.
    /// (BFS sur graphe inversé)
    pub fn ancestors_of(target: &BlobId, max_depth: u32) -> ExofsResult<Vec<BlobId>> {
        use alloc::collections::VecDeque;
        let mut visited: alloc::collections::BTreeMap<[u8; 32], u32> =
            alloc::collections::BTreeMap::new();
        let mut queue: VecDeque<(BlobId, u32)> = VecDeque::new();
        let mut out: Vec<BlobId> = Vec::new();

        visited.insert(*target.as_bytes(), 0);
        queue.push_back((*target, 0));

        while let Some((node, depth)) = queue.pop_front() {
            let incoming_ids = RELATION_INDEX.ids_to(&node);
            for id in incoming_ids {
                if let Some(r) = RELATION_STORAGE.load(id) {
                    let r = r?;
                    if !visited.contains_key(r.from.as_bytes()) {
                        let new_depth = depth.checked_add(1)
                            .ok_or(ExofsError::OffsetOverflow)?;
                        if new_depth <= max_depth {
                            visited.insert(*r.from.as_bytes(), new_depth);
                            out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                            out.push(r.from);
                            queue.push_back((r.from, new_depth));
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// Compte les relations d un blob par direction.
    pub fn count_relations(blob: &BlobId, direction: RelationDirection) -> usize {
        match direction {
            RelationDirection::Outgoing => RELATION_INDEX.out_degree(blob),
            RelationDirection::Incoming => RELATION_INDEX.in_degree(blob),
            RelationDirection::Both     =>
                RELATION_INDEX.out_degree(blob) + RELATION_INDEX.in_degree(blob),
        }
    }

    /// Retourne les neighbors de `blob` d un certain type.
    pub fn neighbors_of_kind(blob: &BlobId, kind: RelationKind) -> Vec<BlobId> {
        RELATION_GRAPH.get_neighbors_by_kind(blob, kind)
    }

    /// Retourne tous les voisins directs.
    pub fn all_neighbors(blob: &BlobId) -> Vec<BlobId> {
        RELATION_GRAPH.get_neighbors(blob)
    }

    /// Charge une relation par son ID.
    pub fn load_by_id(id: RelationId) -> Option<ExofsResult<Relation>> {
        RELATION_STORAGE.load(id)
    }

    /// Charge toutes les relations d un kind donné dans le store.
    pub fn all_of_kind(kind: RelationKind) -> ExofsResult<QueryResult> {
        let rels = RELATION_STORAGE.load_by_kind(kind)?;
        Ok(QueryResult::from_rels(rels))
    }

    // Helper interne : charge les relations depuis une liste d IDs.
    fn load_ids(ids: &[RelationId]) -> ExofsResult<QueryResult> {
        let mut rels: Vec<Relation> = Vec::new();
        rels.try_reserve(ids.len()).map_err(|_| ExofsError::NoMemory)?;
        for &id in ids {
            if let Some(r) = RELATION_STORAGE.load(id) {
                rels.push(r?);
            }
        }
        Ok(QueryResult::from_rels(rels))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// QueryBuilder — API fluente
// ─────────────────────────────────────────────────────────────────────────────

/// Builder pour construire une requête complexe.
pub struct QueryBuilder {
    blob:      BlobId,
    filter:    RelationFilter,
    max_items: Option<usize>,
}

impl QueryBuilder {
    pub fn new(blob: BlobId) -> Self {
        QueryBuilder {
            blob,
            filter:    RelationFilter::default(),
            max_items: None,
        }
    }

    pub fn kind(mut self, k: RelationKind) -> Self {
        self.filter.kind = Some(k); self
    }

    pub fn direction(mut self, d: RelationDirection) -> Self {
        self.filter.direction = Some(d); self
    }

    pub fn active_only(mut self) -> Self {
        self.filter.active_only = true; self
    }

    pub fn min_weight(mut self, w: u32) -> Self {
        self.filter.min_weight = Some(w); self
    }

    pub fn limit(mut self, n: usize) -> Self {
        self.max_items = Some(n); self
    }

    pub fn execute(self) -> ExofsResult<QueryResult> {
        let mut res = RelationQuery::filtered(&self.blob, self.filter)?;
        if let Some(max) = self.max_items {
            res = res.limit(max);
        }
        Ok(res)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::relation_type::RelationType;
    use super::super::relation::RelationId;
    use super::super::relation_storage::RelationStorage;
    use super::super::relation_index::RelationIndex;
    use super::super::relation_graph::RelationGraph;

    fn blob(b: u8) -> BlobId { BlobId([b; 32]) }

    fn setup_rel(
        store: &RelationStorage,
        idx:   &RelationIndex,
        graph: &RelationGraph,
        id:    u64,
        from:  BlobId,
        to:    BlobId,
        kind:  RelationKind,
    ) {
        let rel = Relation::new(
            RelationId(id), from, to,
            RelationType::new(kind), 0,
        );
        store.persist(&rel).unwrap();
        idx.insert(&rel).unwrap();
        graph.add_relation(&rel).unwrap();
    }

    #[test] fn test_query_result_limit() {
        let rels: Vec<Relation> = (1u64..=5).map(|i| {
            Relation::new(RelationId(i), blob(1), blob(2),
                RelationType::new(RelationKind::CrossRef), 0)
        }).collect();
        let qr = QueryResult::from_rels(rels).limit(3);
        assert_eq!(qr.relations.len(), 3);
        assert!(qr.truncated);
    }

    #[test] fn test_query_result_filter() {
        let rels: Vec<Relation> = vec![
            Relation::new(RelationId(1), blob(1), blob(2),
                RelationType::new(RelationKind::Parent), 0),
            Relation::new(RelationId(2), blob(1), blob(3),
                RelationType::new(RelationKind::Clone), 0),
        ];
        let qr = QueryResult::from_rels(rels)
            .filter_by(|r| r.rel_type.kind == RelationKind::Parent);
        assert_eq!(qr.n_total, 1);
    }

    #[test] fn test_builder_executes() {
        let store = RelationStorage::new_const();
        let idx   = RelationIndex::new_const();
        let graph = RelationGraph::new_const();
        // Pas de données : résultat vide.
        let qr = QueryBuilder::new(blob(50))
            .kind(RelationKind::Parent)
            .active_only()
            .execute().unwrap();
        assert_eq!(qr.n_total, 0);
    }

    #[test] fn test_query_result_not_truncated_when_exact() {
        let rels: Vec<Relation> = (1u64..=3).map(|i| {
            Relation::new(RelationId(i), blob(1), blob(2),
                RelationType::new(RelationKind::CrossRef), 0)
        }).collect();
        let qr = QueryResult::from_rels(rels).limit(10);
        assert!(!qr.truncated);
    }

    #[test] fn test_query_result_empty() {
        let qr = QueryResult::empty();
        assert_eq!(qr.n_total, 0);
        assert!(!qr.truncated);
        assert!(qr.relations.is_empty());
    }

    #[test] fn test_query_result_filter_all_out() {
        let rels: Vec<Relation> = vec![
            Relation::new(RelationId(10), blob(1), blob(2),
                RelationType::new(RelationKind::Dedup), 0),
        ];
        let qr = QueryResult::from_rels(rels)
            .filter_by(|r| r.rel_type.kind == RelationKind::Parent);
        assert_eq!(qr.n_total, 0);
        assert!(qr.relations.is_empty());
    }

    #[test] fn test_reachable_from_empty_graph() {
        let qr = RelationQuery::reachable_from(&blob(200), 4).unwrap();
        assert_eq!(qr.n_total, 0);
    }

    #[test] fn test_ancestors_of_empty_graph() {
        let qr = RelationQuery::ancestors_of(&blob(201), 4).unwrap();
        assert_eq!(qr.n_total, 0);
    }

    #[test] fn test_all_of_kind_empty() {
        let qr = RelationQuery::all_of_kind(RelationKind::Snapshot).unwrap();
        // Le store global peut contenir d'autres relations, mais la fonction
        // ne doit pas paniquer.
        let _ = qr;
    }

    #[test] fn test_builder_no_kind() {
        let qr = QueryBuilder::new(blob(250)).execute().unwrap();
        let _ = qr; // Résultat vide ou non — ne doit pas paniquer.
    }

    #[test] fn test_builder_max_depth() {
        let qr = QueryBuilder::new(blob(251))
            .max_depth(2)
            .execute().unwrap();
        let _ = qr;
    }

    #[test] fn test_query_result_multiple_limits() {
        let rels: Vec<Relation> = (1u64..=20).map(|i| {
            Relation::new(RelationId(i), blob(2), blob(3),
                RelationType::new(RelationKind::HardLink), 0)
        }).collect();
        let qr = QueryResult::from_rels(rels).limit(5);
        assert_eq!(qr.relations.len(), 5);
        assert!(qr.truncated);
        assert_eq!(qr.n_total, 5);
    }
}
