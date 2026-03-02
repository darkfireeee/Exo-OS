//! RelationIndex — index rapide des relations par BlobId ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use super::relation::{Relation, RelationId};

pub static RELATION_INDEX: RelationIndex = RelationIndex::new_const();

pub struct RelationIndex {
    /// by_from : from_blob → Vec<RelationId>
    by_from: SpinLock<BTreeMap<[u8; 32], Vec<RelationId>>>,
    /// by_to : to_blob → Vec<RelationId>
    by_to:   SpinLock<BTreeMap<[u8; 32], Vec<RelationId>>>,
}

impl RelationIndex {
    pub const fn new_const() -> Self {
        Self {
            by_from: SpinLock::new(BTreeMap::new()),
            by_to:   SpinLock::new(BTreeMap::new()),
        }
    }

    fn insert_into(
        map: &SpinLock<BTreeMap<[u8; 32], Vec<RelationId>>>,
        key: [u8; 32],
        id:  RelationId,
    ) -> Result<(), FsError> {
        let mut guard = map.lock();
        let vec = if let Some(v) = guard.get_mut(&key) {
            v
        } else {
            guard.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            guard.insert(key, Vec::new());
            guard.get_mut(&key).unwrap()
        };
        vec.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        vec.push(id);
        Ok(())
    }

    fn remove_from(
        map: &SpinLock<BTreeMap<[u8; 32], Vec<RelationId>>>,
        key: [u8; 32],
        id:  RelationId,
    ) {
        let mut guard = map.lock();
        if let Some(v) = guard.get_mut(&key) {
            v.retain(|r| *r != id);
            if v.is_empty() { guard.remove(&key); }
        }
    }

    pub fn insert(&self, rel: &Relation) -> Result<(), FsError> {
        Self::insert_into(&self.by_from, rel.from.as_bytes(), rel.id)?;
        Self::insert_into(&self.by_to,   rel.to.as_bytes(),   rel.id)?;
        Ok(())
    }

    pub fn remove(&self, rel: &Relation) {
        Self::remove_from(&self.by_from, rel.from.as_bytes(), rel.id);
        Self::remove_from(&self.by_to,   rel.to.as_bytes(),   rel.id);
    }

    pub fn ids_from(&self, from: &BlobId) -> Vec<RelationId> {
        self.by_from.lock().get(&from.as_bytes()).cloned().unwrap_or_default()
    }

    pub fn ids_to(&self, to: &BlobId) -> Vec<RelationId> {
        self.by_to.lock().get(&to.as_bytes()).cloned().unwrap_or_default()
    }

    pub fn has_from(&self, blob: &BlobId) -> bool {
        self.by_from.lock().contains_key(&blob.as_bytes())
    }
}
