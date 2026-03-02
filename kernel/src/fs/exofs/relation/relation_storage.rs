//! RelationStorage — persistance on-disk des relations ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use super::relation::{Relation, RelationId, RelationOnDisk};

pub static RELATION_STORAGE: RelationStorage = RelationStorage::new_const();

pub struct RelationStorage {
    store: SpinLock<BTreeMap<u64, RelationOnDisk>>,
    next_id: core::sync::atomic::AtomicU64,
}

impl RelationStorage {
    pub const fn new_const() -> Self {
        Self {
            store:   SpinLock::new(BTreeMap::new()),
            next_id: core::sync::atomic::AtomicU64::new(1),
        }
    }

    pub fn allocate_id(&self) -> RelationId {
        RelationId(self.next_id.fetch_add(1, core::sync::atomic::Ordering::Relaxed))
    }

    pub fn persist(&self, rel: &Relation) -> Result<(), FsError> {
        let on_disk = rel.to_on_disk();
        let mut store = self.store.lock();
        store.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        store.insert(rel.id.0, on_disk);
        Ok(())
    }

    pub fn remove(&self, id: RelationId) -> bool {
        self.store.lock().remove(&id.0).is_some()
    }

    pub fn load(&self, id: RelationId) -> Option<Relation> {
        self.store.lock().get(&id.0).map(Relation::from_on_disk)
    }

    pub fn load_all(&self) -> Result<Vec<Relation>, FsError> {
        let store = self.store.lock();
        let mut out = Vec::new();
        out.try_reserve(store.len()).map_err(|_| FsError::OutOfMemory)?;
        for d in store.values() {
            out.push(Relation::from_on_disk(d));
        }
        Ok(out)
    }

    pub fn count(&self) -> usize { self.store.lock().len() }
}
