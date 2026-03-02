//! RelationGraph — graphe d'adjacence des relations ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use super::relation::{Relation, RelationId};
use super::relation_type::RelationKind;

pub static RELATION_GRAPH: RelationGraph = RelationGraph::new_const();

/// Arête dans le graphe.
#[derive(Clone, Debug)]
struct Edge {
    id:   RelationId,
    to:   BlobId,
    kind: RelationKind,
}

pub struct RelationGraph {
    /// adj[from] = Vec<Edge>
    adj: SpinLock<BTreeMap<[u8; 32], Vec<Edge>>>,
    /// Compteur total d'arêtes.
    n_edges: core::sync::atomic::AtomicU64,
}

impl RelationGraph {
    pub const fn new_const() -> Self {
        Self {
            adj:     SpinLock::new(BTreeMap::new()),
            n_edges: core::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn add_relation(&self, rel: &Relation) -> Result<(), FsError> {
        let from_key = rel.from.as_bytes();
        let edge = Edge { id: rel.id, to: rel.to, kind: rel.rel_type.kind };

        let mut adj = self.adj.lock();
        let vec = if let Some(v) = adj.get_mut(&from_key) {
            v
        } else {
            adj.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            adj.insert(from_key, Vec::new());
            adj.get_mut(&from_key).unwrap()
        };
        vec.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        vec.push(edge);
        self.n_edges.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub fn remove_relation(&self, rel: &Relation) {
        let from_key = rel.from.as_bytes();
        let mut adj = self.adj.lock();
        if let Some(vec) = adj.get_mut(&from_key) {
            vec.retain(|e| e.id != rel.id);
            if vec.is_empty() { adj.remove(&from_key); }
            self.n_edges.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn get_neighbors(&self, from: &BlobId) -> Vec<BlobId> {
        let adj = self.adj.lock();
        adj.get(&from.as_bytes())
            .map(|v| v.iter().map(|e| e.to).collect())
            .unwrap_or_default()
    }

    pub fn get_neighbors_by_kind(&self, from: &BlobId, kind: RelationKind) -> Vec<BlobId> {
        let adj = self.adj.lock();
        adj.get(&from.as_bytes())
            .map(|v| v.iter().filter(|e| e.kind == kind).map(|e| e.to).collect())
            .unwrap_or_default()
    }

    pub fn out_degree(&self, from: &BlobId) -> usize {
        self.adj.lock().get(&from.as_bytes()).map(|v| v.len()).unwrap_or(0)
    }

    pub fn n_edges(&self) -> u64 { self.n_edges.load(core::sync::atomic::Ordering::Relaxed) }
    pub fn n_nodes(&self) -> usize { self.adj.lock().len() }
}
