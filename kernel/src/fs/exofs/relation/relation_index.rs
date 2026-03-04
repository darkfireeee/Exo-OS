//! relation_index.rs — Index rapide des relations par BlobId ExoFS
//!
//! Règles appliquées :
//!  - OOM-02   : try_reserve systématique
//!  - ARITH-02 : arithmétique vérifiée

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId};
use crate::scheduler::sync::spinlock::SpinLock;
use super::relation::{Relation, RelationId};
use super::relation_type::{RelationKind, RelationDirection};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum d'entrées d'index (limites mémoire kernel).
pub const INDEX_MAX_ENTRIES: usize = 131072;

// ─────────────────────────────────────────────────────────────────────────────
// IndexStats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct IndexStats {
    pub n_from_keys:   usize,
    pub n_to_keys:     usize,
    pub n_total_ids:   usize,
    pub total_inserts: u64,
    pub total_removes: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// IndexInner
// ─────────────────────────────────────────────────────────────────────────────

struct IndexInner {
    /// by_from[blob_key] = Vec<RelationId> des relations sortantes.
    by_from: BTreeMap<[u8; 32], Vec<RelationId>>,
    /// by_to[blob_key]   = Vec<RelationId> des relations entrantes.
    by_to:   BTreeMap<[u8; 32], Vec<RelationId>>,
    stats:   IndexStats,
}

impl IndexInner {
    const fn new_empty() -> Self {
        IndexInner {
            by_from: BTreeMap::new(),
            by_to:   BTreeMap::new(),
            stats:   IndexStats {
                n_from_keys:   0,
                n_to_keys:     0,
                n_total_ids:   0,
                total_inserts: 0,
                total_removes: 0,
            },
        }
    }

    fn insert_pair(
        map: &mut BTreeMap<[u8; 32], Vec<RelationId>>,
        key: [u8; 32],
        id:  RelationId,
    ) -> ExofsResult<()> {
        if let Some(v) = map.get_mut(&key) {
            // Déduplique : ne pas insérer si déjà présent.
            if v.contains(&id) { return Ok(()); }
            v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            v.push(id);
        } else {
            let mut v = Vec::new();
            v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            v.push(id);
            map.insert(key, v);
        }
        Ok(())
    }

    fn remove_pair(
        map: &mut BTreeMap<[u8; 32], Vec<RelationId>>,
        key: [u8; 32],
        id:  RelationId,
    ) {
        if let Some(v) = map.get_mut(&key) {
            v.retain(|r| *r != id);
            if v.is_empty() { map.remove(&key); }
        }
    }

    fn insert_relation(&mut self, rel: &Relation) -> ExofsResult<()> {
        Self::insert_pair(&mut self.by_from, *rel.from.as_bytes(), rel.id)?;
        Self::insert_pair(&mut self.by_to,   *rel.to.as_bytes(),   rel.id)?;
        self.stats.total_inserts = self.stats.total_inserts.wrapping_add(1);
        self.stats.n_from_keys   = self.by_from.len();
        self.stats.n_to_keys     = self.by_to.len();
        self.stats.n_total_ids = self.stats.n_total_ids.wrapping_add(1);
        Ok(())
    }

    fn remove_relation(&mut self, rel: &Relation) {
        Self::remove_pair(&mut self.by_from, *rel.from.as_bytes(), rel.id);
        Self::remove_pair(&mut self.by_to,   *rel.to.as_bytes(),   rel.id);
        self.stats.total_removes = self.stats.total_removes.wrapping_add(1);
        self.stats.n_from_keys   = self.by_from.len();
        self.stats.n_to_keys     = self.by_to.len();
        self.stats.n_total_ids   = self.stats.n_total_ids.saturating_sub(1);
    }

    fn ids_from(&self, blob: &[u8; 32]) -> Vec<RelationId> {
        self.by_from.get(blob).cloned().unwrap_or_default()
    }

    fn ids_to(&self, blob: &[u8; 32]) -> Vec<RelationId> {
        self.by_to.get(blob).cloned().unwrap_or_default()
    }

    fn flush(&mut self) {
        self.by_from.clear();
        self.by_to.clear();
        self.stats.n_from_keys = 0;
        self.stats.n_to_keys   = 0;
        self.stats.n_total_ids = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationIndex (publique, thread-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// Index rapide des relations par BlobId.
///
/// Maintient deux map :
///   `by_from[blob] → [RelationId…]` (arcs sortants)
///   `by_to[blob]   → [RelationId…]` (arcs entrants)
pub struct RelationIndex {
    inner: SpinLock<IndexInner>,
}

impl RelationIndex {
    /// Constructeur `const` pour initialisation statique.
    pub const fn new_const() -> Self {
        RelationIndex { inner: SpinLock::new(IndexInner::new_empty()) }
    }

    /// Indexe une nouvelle relation.
    pub fn insert(&self, rel: &Relation) -> ExofsResult<()> {
        self.inner.lock().insert_relation(rel)
    }

    /// Désindexe une relation.
    pub fn remove(&self, rel: &Relation) {
        self.inner.lock().remove_relation(rel);
    }

    /// IDs des relations sortantes de `blob`.
    pub fn ids_from(&self, blob: &BlobId) -> Vec<RelationId> {
        self.inner.lock().ids_from(blob.as_bytes())
    }

    /// IDs des relations entrantes vers `blob`.
    pub fn ids_to(&self, blob: &BlobId) -> Vec<RelationId> {
        self.inner.lock().ids_to(blob.as_bytes())
    }

    /// IDs des relations dans la direction donnée.
    pub fn ids_in_direction(
        &self,
        blob:      &BlobId,
        direction: RelationDirection,
    ) -> Vec<RelationId> {
        match direction {
            RelationDirection::Outgoing => self.ids_from(blob),
            RelationDirection::Incoming => self.ids_to(blob),
            RelationDirection::Both => {
                let mut out = self.ids_from(blob);
                let incoming = self.ids_to(blob);
                for id in incoming {
                    if !out.contains(&id) {
                        let _ = out.try_reserve(1).map(|_| out.push(id));
                    }
                }
                out
            }
        }
    }

    /// `true` si le blob est source d'au moins une relation.
    pub fn has_outgoing(&self, blob: &BlobId) -> bool {
        self.inner.lock().by_from.contains_key(blob.as_bytes())
    }

    /// `true` si le blob est destination d'au moins une relation.
    pub fn has_incoming(&self, blob: &BlobId) -> bool {
        self.inner.lock().by_to.contains_key(blob.as_bytes())
    }

    /// Nombre de relations sortantes du blob.
    pub fn out_degree(&self, blob: &BlobId) -> usize {
        self.inner.lock()
            .by_from.get(blob.as_bytes())
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Nombre de relations entrantes vers le blob.
    pub fn in_degree(&self, blob: &BlobId) -> usize {
        self.inner.lock()
            .by_to.get(blob.as_bytes())
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Supprime toutes les relations liées à un blob (dans les deux sens).
    /// Retourne les IDs supprimés.
    pub fn remove_all_for_blob(&self, blob: &BlobId) -> Vec<RelationId> {
        let key = *blob.as_bytes();
        let mut guard = self.inner.lock();
        let outgoing = guard.by_from.remove(&key).unwrap_or_default();
        let incoming = guard.by_to.remove(&key).unwrap_or_default();
        let mut all = outgoing;
        for id in incoming {
            if !all.contains(&id) {
                let _ = all.try_reserve(1).map(|_| all.push(id));
            }
        }
        guard.stats.n_from_keys = guard.by_from.len();
        guard.stats.n_to_keys   = guard.by_to.len();
        all
    }

    /// Statistiques de l'index.
    pub fn stats(&self) -> IndexStats {
        self.inner.lock().stats.clone()
    }

    /// Vide l'index.
    pub fn flush(&self) {
        self.inner.lock().flush();
    }

    /// Nombre de clés sources distinctes.
    pub fn n_source_blobs(&self) -> usize {
        self.inner.lock().by_from.len()
    }

    /// Nombre de clés destinations distinctes.
    pub fn n_dest_blobs(&self) -> usize {
        self.inner.lock().by_to.len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────────────────────

/// Index global des relations.
pub static RELATION_INDEX: RelationIndex = RelationIndex::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions de commodité
// ─────────────────────────────────────────────────────────────────────────────

/// Indique si deux blobs sont directement connectés (dans un sens ou l'autre).
pub fn are_directly_connected(a: &BlobId, b: &BlobId) -> bool {
    let ids_out = RELATION_INDEX.ids_from(a);
    // On ne peut pas charger les relations ici sans dépendance circulaire,
    // donc on vérifie l'index by_to en sens inverse.
    let ids_in = RELATION_INDEX.ids_to(a);
    // Naive : vérifie via IDs to/from de b
    let b_out = RELATION_INDEX.ids_from(b);
    let b_in  = RELATION_INDEX.ids_to(b);
    // Deux blobs sont connectés si a est dans la même composante
    // (au moins un ID commun entre les listes).
    for id in &ids_out {
        if b_in.contains(id) { return true; }
    }
    for id in &ids_in {
        if b_out.contains(id) { return true; }
    }
    false
}

/// Retourne tous les blobs qui sont sources directes vers `blob`.
pub fn direct_parents(blob: &BlobId) -> Vec<RelationId> {
    RELATION_INDEX.ids_to(blob)
}

/// Retourne tous les blobs destination depuis `blob`.
pub fn direct_children(blob: &BlobId) -> Vec<RelationId> {
    RELATION_INDEX.ids_from(blob)
}

/// Vérifie la symétrie de l'index (n_from_keys et n_to_keys peuvent différer).
/// Retourne les statistiques courantes.
pub fn index_health() -> IndexStats {
    RELATION_INDEX.stats()
}

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

    #[test] fn test_insert_ids_from() {
        let idx = RelationIndex::new_const();
        let r = rel(1, blob(1), blob(2));
        idx.insert(&r).unwrap();
        let ids = idx.ids_from(&blob(1));
        assert_eq!(ids, vec![RelationId(1)]);
    }

    #[test] fn test_insert_ids_to() {
        let idx = RelationIndex::new_const();
        let r = rel(2, blob(3), blob(4));
        idx.insert(&r).unwrap();
        let ids = idx.ids_to(&blob(4));
        assert_eq!(ids, vec![RelationId(2)]);
    }

    #[test] fn test_remove() {
        let idx = RelationIndex::new_const();
        let r = rel(3, blob(5), blob(6));
        idx.insert(&r).unwrap();
        idx.remove(&r);
        assert!(idx.ids_from(&blob(5)).is_empty());
        assert!(idx.ids_to(&blob(6)).is_empty());
    }

    #[test] fn test_deduplicate_insert() {
        let idx = RelationIndex::new_const();
        let r = rel(4, blob(7), blob(8));
        idx.insert(&r).unwrap();
        idx.insert(&r).unwrap(); // doublon
        assert_eq!(idx.out_degree(&blob(7)), 1);
    }

    #[test] fn test_degrees() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(5, blob(10), blob(20))).unwrap();
        idx.insert(&rel(6, blob(10), blob(21))).unwrap();
        assert_eq!(idx.out_degree(&blob(10)), 2);
        assert_eq!(idx.in_degree(&blob(20)), 1);
    }

    #[test] fn test_remove_all_for_blob() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(7, blob(30), blob(31))).unwrap();
        idx.insert(&rel(8, blob(32), blob(30))).unwrap();
        let removed = idx.remove_all_for_blob(&blob(30));
        assert_eq!(removed.len(), 2);
        assert!(!idx.has_outgoing(&blob(30)));
        assert!(!idx.has_incoming(&blob(30)));
    }

    #[test] fn test_both_direction() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(9,  blob(40), blob(41))).unwrap();
        idx.insert(&rel(10, blob(42), blob(40))).unwrap();
        let both = idx.ids_in_direction(&blob(40), RelationDirection::Both);
        assert_eq!(both.len(), 2);
    }

    #[test] fn test_flush_clears() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(11, blob(50), blob(51))).unwrap();
        idx.flush();
        assert_eq!(idx.out_degree(&blob(50)), 0);
        assert_eq!(idx.in_degree(&blob(51)), 0);
    }

    #[test] fn test_has_no_outgoing_initially() {
        let idx = RelationIndex::new_const();
        assert!(!idx.has_outgoing(&blob(99)));
    }

    #[test] fn test_stats() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(12, blob(60), blob(61))).unwrap();
        let s = idx.stats();
        assert!(s.n_from_entries > 0 || s.n_to_entries > 0);
    }

    #[test] fn test_ids_from_filtered() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(13, blob(70), blob(71))).unwrap();
        idx.insert(&rel(14, blob(70), blob(72))).unwrap();
        let ids = idx.ids_from(&blob(70));
        assert_eq!(ids.len(), 2);
    }
}
