//! relation_index.rs — Fast relation index by BlobId in ExoFS
//!
//! Applied rules:
//!  - OOM-02   : systematic try_reserve
//!  - ARITH-02 : checked arithmetic

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::relation::{Relation, RelationId};
use super::relation_type::RelationDirection;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of index entries (kernel memory limits).
pub const INDEX_MAX_ENTRIES: usize = 131072;

// ─────────────────────────────────────────────────────────────────────────────
// IndexStats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct IndexStats {
    pub n_from_keys: usize,
    pub n_to_keys: usize,
    pub n_total_ids: usize,
    pub total_inserts: u64,
    pub total_removes: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// IndexInner
// ─────────────────────────────────────────────────────────────────────────────

struct IndexInner {
    /// by_from[blob_key] = Vec<RelationId> of outgoing relations.
    by_from: BTreeMap<[u8; 32], Vec<RelationId>>,
    /// by_to[blob_key]   = Vec<RelationId> of incoming relations.
    by_to: BTreeMap<[u8; 32], Vec<RelationId>>,
    stats: IndexStats,
}

impl IndexInner {
    const fn new_empty() -> Self {
        IndexInner {
            by_from: BTreeMap::new(),
            by_to: BTreeMap::new(),
            stats: IndexStats {
                n_from_keys: 0,
                n_to_keys: 0,
                n_total_ids: 0,
                total_inserts: 0,
                total_removes: 0,
            },
        }
    }

    fn insert_pair(
        map: &mut BTreeMap<[u8; 32], Vec<RelationId>>,
        key: [u8; 32],
        id: RelationId,
    ) -> ExofsResult<()> {
        if let Some(v) = map.get_mut(&key) {
            // Deduplicate: do not insert if already present.
            if v.contains(&id) {
                return Ok(());
            }
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

    fn remove_pair(map: &mut BTreeMap<[u8; 32], Vec<RelationId>>, key: [u8; 32], id: RelationId) {
        if let Some(v) = map.get_mut(&key) {
            v.retain(|r| *r != id);
            if v.is_empty() {
                map.remove(&key);
            }
        }
    }

    fn insert_relation(&mut self, rel: &Relation) -> ExofsResult<()> {
        Self::insert_pair(&mut self.by_from, *rel.from.as_bytes(), rel.id)?;
        Self::insert_pair(&mut self.by_to, *rel.to.as_bytes(), rel.id)?;
        self.stats.total_inserts = self.stats.total_inserts.wrapping_add(1);
        self.stats.n_from_keys = self.by_from.len();
        self.stats.n_to_keys = self.by_to.len();
        self.stats.n_total_ids = self.stats.n_total_ids.wrapping_add(1);
        Ok(())
    }

    fn remove_relation(&mut self, rel: &Relation) {
        Self::remove_pair(&mut self.by_from, *rel.from.as_bytes(), rel.id);
        Self::remove_pair(&mut self.by_to, *rel.to.as_bytes(), rel.id);
        self.stats.total_removes = self.stats.total_removes.wrapping_add(1);
        self.stats.n_from_keys = self.by_from.len();
        self.stats.n_to_keys = self.by_to.len();
        self.stats.n_total_ids = self.stats.n_total_ids.saturating_sub(1);
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
        self.stats.n_to_keys = 0;
        self.stats.n_total_ids = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RelationIndex (public, thread-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// Fast relation index by BlobId.
///
/// Maintains two maps:
///   `by_from[blob] -> [RelationId...]` (outgoing edges)
///   `by_to[blob]   -> [RelationId...]` (incoming edges)
pub struct RelationIndex {
    inner: SpinLock<IndexInner>,
}

impl RelationIndex {
    /// `const` constructor for static initialization.
    pub const fn new_const() -> Self {
        RelationIndex {
            inner: SpinLock::new(IndexInner::new_empty()),
        }
    }

    /// Indexes a new relation.
    pub fn insert(&self, rel: &Relation) -> ExofsResult<()> {
        self.inner.lock().insert_relation(rel)
    }

    /// Unindexes a relation.
    pub fn remove(&self, rel: &Relation) {
        self.inner.lock().remove_relation(rel);
    }

    /// IDs of outgoing relations from `blob`.
    pub fn ids_from(&self, blob: &BlobId) -> Vec<RelationId> {
        self.inner.lock().ids_from(blob.as_bytes())
    }

    /// IDs of incoming relations towards `blob`.
    pub fn ids_to(&self, blob: &BlobId) -> Vec<RelationId> {
        self.inner.lock().ids_to(blob.as_bytes())
    }

    /// IDs of relations in given direction.
    pub fn ids_in_direction(&self, blob: &BlobId, direction: RelationDirection) -> Vec<RelationId> {
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

    /// `true` if the blob is source of at least one relation.
    pub fn has_outgoing(&self, blob: &BlobId) -> bool {
        self.inner.lock().by_from.contains_key(blob.as_bytes())
    }

    /// `true` if the blob is destination of at least one relation.
    pub fn has_incoming(&self, blob: &BlobId) -> bool {
        self.inner.lock().by_to.contains_key(blob.as_bytes())
    }

    /// Number of outgoing relations from the blob.
    pub fn out_degree(&self, blob: &BlobId) -> usize {
        self.inner
            .lock()
            .by_from
            .get(blob.as_bytes())
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Number of incoming relations towards the blob.
    pub fn in_degree(&self, blob: &BlobId) -> usize {
        self.inner
            .lock()
            .by_to
            .get(blob.as_bytes())
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Removes all relations linked to a blob (in both directions).
    /// Returns removed IDs.
    pub fn remove_all_for_blob(&self, blob: &BlobId) -> Vec<RelationId> {
        let key = *blob.as_bytes();
        let mut guard = self.inner.lock();
        let outgoing = guard.by_from.remove(&key).unwrap_or_default();
        let incoming = guard.by_to.remove(&key).unwrap_or_default();

        // Clean outgoing relations of this blob in peer `by_to`
        for id in &outgoing {
            for v in guard.by_to.values_mut() {
                v.retain(|r| r != id);
            }
        }
        guard.by_to.retain(|_, v| !v.is_empty());

        // Clean incoming relations towards this blob in peer `by_from`
        for id in &incoming {
            for v in guard.by_from.values_mut() {
                v.retain(|r| r != id);
            }
        }
        guard.by_from.retain(|_, v| !v.is_empty());

        let mut all = outgoing;
        for id in incoming {
            if !all.contains(&id) {
                let _ = all.try_reserve(1).map(|_| all.push(id));
            }
        }
        guard.stats.n_from_keys = guard.by_from.len();
        guard.stats.n_to_keys = guard.by_to.len();
        guard.stats.n_total_ids = guard.stats.n_total_ids.saturating_sub(all.len());
        guard.stats.total_removes = guard.stats.total_removes.wrapping_add(all.len() as u64);
        all
    }

    /// Index statistics.
    pub fn stats(&self) -> IndexStats {
        self.inner.lock().stats.clone()
    }

    /// Clears the index.
    pub fn flush(&self) {
        self.inner.lock().flush();
    }

    /// Number of distinct source keys.
    pub fn n_source_blobs(&self) -> usize {
        self.inner.lock().by_from.len()
    }

    /// Number of distinct destination keys.
    pub fn n_dest_blobs(&self) -> usize {
        self.inner.lock().by_to.len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global singleton
// ─────────────────────────────────────────────────────────────────────────────

/// Global relations index.
pub static RELATION_INDEX: RelationIndex = RelationIndex::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Convenience functions
// ─────────────────────────────────────────────────────────────────────────────

/// Indicates whether two blobs are directly connected (in either direction).
pub fn are_directly_connected(a: &BlobId, b: &BlobId) -> bool {
    let ids_out = RELATION_INDEX.ids_from(a);
    // Cannot load relations here without circular dependency,
    // so verify by_to index in reverse direction.
    let ids_in = RELATION_INDEX.ids_to(a);
    // Naive: check via IDs to/from of b
    let b_out = RELATION_INDEX.ids_from(b);
    let b_in = RELATION_INDEX.ids_to(b);
    // Two blobs are connected if a is in the same component
    // (at least one common ID between the lists).
    for id in &ids_out {
        if b_in.contains(id) {
            return true;
        }
    }
    for id in &ids_in {
        if b_out.contains(id) {
            return true;
        }
    }
    false
}

/// Returns all blobs that are direct sources towards `blob`.
pub fn direct_parents(blob: &BlobId) -> Vec<RelationId> {
    RELATION_INDEX.ids_to(blob)
}

/// Returns all destination blobs from `blob`.
pub fn direct_children(blob: &BlobId) -> Vec<RelationId> {
    RELATION_INDEX.ids_from(blob)
}

/// Checks index symmetry (n_from_keys and n_to_keys may differ).
/// Returns current statistics.
pub fn index_health() -> IndexStats {
    RELATION_INDEX.stats()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::relation_type::{RelationKind, RelationType};
    use super::*;

    fn blob(b: u8) -> BlobId {
        BlobId([b; 32])
    }

    fn rel(id: u64, from: BlobId, to: BlobId) -> Relation {
        Relation::new(
            RelationId(id),
            from,
            to,
            RelationType::new(RelationKind::Parent),
            0,
        )
    }

    #[test]
    fn test_insert_ids_from() {
        let idx = RelationIndex::new_const();
        let r = rel(1, blob(1), blob(2));
        idx.insert(&r).test_unwrap();
        let ids = idx.ids_from(&blob(1));
        assert_eq!(ids, vec![RelationId(1)]);
    }

    #[test]
    fn test_insert_ids_to() {
        let idx = RelationIndex::new_const();
        let r = rel(2, blob(3), blob(4));
        idx.insert(&r).test_unwrap();
        let ids = idx.ids_to(&blob(4));
        assert_eq!(ids, vec![RelationId(2)]);
    }

    #[test]
    fn test_remove() {
        let idx = RelationIndex::new_const();
        let r = rel(3, blob(5), blob(6));
        idx.insert(&r).test_unwrap();
        idx.remove(&r);
        assert!(idx.ids_from(&blob(5)).is_empty());
        assert!(idx.ids_to(&blob(6)).is_empty());
    }

    #[test]
    fn test_deduplicate_insert() {
        let idx = RelationIndex::new_const();
        let r = rel(4, blob(7), blob(8));
        idx.insert(&r).test_unwrap();
        idx.insert(&r).test_unwrap(); // duplicate
        assert_eq!(idx.out_degree(&blob(7)), 1);
    }

    #[test]
    fn test_degrees() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(5, blob(10), blob(20))).test_unwrap();
        idx.insert(&rel(6, blob(10), blob(21))).test_unwrap();
        assert_eq!(idx.out_degree(&blob(10)), 2);
        assert_eq!(idx.in_degree(&blob(20)), 1);
    }

    #[test]
    fn test_remove_all_for_blob() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(7, blob(30), blob(31))).test_unwrap();
        idx.insert(&rel(8, blob(32), blob(30))).test_unwrap();
        let removed = idx.remove_all_for_blob(&blob(30));
        assert_eq!(removed.len(), 2);
        assert!(!idx.has_outgoing(&blob(30)));
        assert!(!idx.has_incoming(&blob(30)));
        assert!(!idx.has_incoming(&blob(31)));
        assert!(!idx.has_outgoing(&blob(32)));
    }

    #[test]
    fn test_both_direction() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(9, blob(40), blob(41))).test_unwrap();
        idx.insert(&rel(10, blob(42), blob(40))).test_unwrap();
        let both = idx.ids_in_direction(&blob(40), RelationDirection::Both);
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn test_flush_clears() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(11, blob(50), blob(51))).test_unwrap();
        idx.flush();
        assert_eq!(idx.out_degree(&blob(50)), 0);
        assert_eq!(idx.in_degree(&blob(51)), 0);
    }

    #[test]
    fn test_has_no_outgoing_initially() {
        let idx = RelationIndex::new_const();
        assert!(!idx.has_outgoing(&blob(99)));
    }

    #[test]
    fn test_stats() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(12, blob(60), blob(61))).test_unwrap();
        let s = idx.stats();
        assert!(s.n_from_keys > 0 || s.n_to_keys > 0);
    }

    #[test]
    fn test_ids_from_filtered() {
        let idx = RelationIndex::new_const();
        idx.insert(&rel(13, blob(70), blob(71))).test_unwrap();
        idx.insert(&rel(14, blob(70), blob(72))).test_unwrap();
        let ids = idx.ids_from(&blob(70));
        assert_eq!(ids.len(), 2);
    }
}
