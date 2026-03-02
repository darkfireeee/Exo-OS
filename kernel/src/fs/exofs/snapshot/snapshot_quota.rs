//! SnapshotQuota — quota d'espace par snapshot ExoFS (no_std).

use alloc::collections::BTreeMap;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;
use super::snapshot::{SnapshotId};
use super::snapshot_list::SNAPSHOT_LIST;

pub static SNAPSHOT_QUOTA: SnapshotQuota = SnapshotQuota::new_const();

#[derive(Clone, Copy, Debug)]
pub struct SnapshotQuotaEntry {
    pub snap_id:     SnapshotId,
    pub max_bytes:   u64,
    pub max_blobs:   u64,
}

pub struct SnapshotQuota {
    entries: SpinLock<BTreeMap<u64, SnapshotQuotaEntry>>,
}

impl SnapshotQuota {
    pub const fn new_const() -> Self {
        Self { entries: SpinLock::new(BTreeMap::new()) }
    }

    pub fn set_quota(
        &self,
        snap_id:   SnapshotId,
        max_bytes: u64,
        max_blobs: u64,
    ) -> Result<(), FsError> {
        let mut entries = self.entries.lock();
        entries.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        entries.insert(snap_id.0, SnapshotQuotaEntry { snap_id, max_bytes, max_blobs });
        Ok(())
    }

    pub fn remove_quota(&self, snap_id: SnapshotId) {
        self.entries.lock().remove(&snap_id.0);
    }

    /// Vérifie si l'ajout de `delta_bytes` blobs dépasse le quota.
    pub fn check_write(
        &self,
        snap_id:     SnapshotId,
        delta_bytes: u64,
        delta_blobs: u64,
    ) -> Result<(), FsError> {
        let entries = self.entries.lock();
        if let Some(q) = entries.get(&snap_id.0) {
            let snap = SNAPSHOT_LIST.get(snap_id).ok_or(FsError::NotFound)?;
            let new_bytes = snap.total_bytes.saturating_add(delta_bytes);
            let new_blobs = snap.n_blobs.saturating_add(delta_blobs);
            if q.max_bytes > 0 && new_bytes > q.max_bytes { return Err(FsError::Overflow); }
            if q.max_blobs > 0 && new_blobs > q.max_blobs { return Err(FsError::Overflow); }
        }
        Ok(())
    }

    pub fn get(&self, snap_id: SnapshotId) -> Option<SnapshotQuotaEntry> {
        self.entries.lock().get(&snap_id.0).copied()
    }
}
