//! SnapshotList — registre mémoire des snapshots ExoFS (no_std).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;
use super::snapshot::{Snapshot, SnapshotId};

pub static SNAPSHOT_LIST: SnapshotList = SnapshotList::new_const();

pub struct SnapshotList {
    pub(super) snapshots: SpinLock<BTreeMap<u64, Snapshot>>,
    next_id:              AtomicU64,
    count:                AtomicU64,
}

impl SnapshotList {
    pub const fn new_const() -> Self {
        Self {
            snapshots: SpinLock::new(BTreeMap::new()),
            next_id:   AtomicU64::new(1),
            count:     AtomicU64::new(0),
        }
    }

    pub fn allocate_id(&self) -> SnapshotId {
        SnapshotId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    pub fn register(&self, snap: Snapshot) -> Result<(), FsError> {
        let mut list = self.snapshots.lock();
        list.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        list.insert(snap.id.0, snap);
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn remove(&self, id: SnapshotId) -> bool {
        let removed = self.snapshots.lock().remove(&id.0).is_some();
        if removed { self.count.fetch_sub(1, Ordering::Relaxed); }
        removed
    }

    pub fn get(&self, id: SnapshotId) -> Option<Snapshot> {
        self.snapshots.lock().get(&id.0).cloned()
    }

    pub fn all_ids(&self) -> Vec<SnapshotId> {
        self.snapshots.lock().keys().map(|&k| SnapshotId(k)).collect()
    }

    pub fn count(&self) -> u64 { self.count.load(Ordering::Relaxed) }

    pub fn total_bytes(&self) -> u64 {
        self.snapshots.lock().values().map(|s| s.total_bytes).sum()
    }

    pub fn is_protected(&self, id: SnapshotId) -> bool {
        self.snapshots.lock().get(&id.0).map_or(false, |s| s.is_protected())
    }

    pub fn set_total_bytes(&self, id: SnapshotId, bytes: u64) {
        if let Some(s) = self.snapshots.lock().get_mut(&id.0) {
            s.total_bytes = bytes;
        }
    }
}
