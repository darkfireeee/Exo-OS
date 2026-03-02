//! SnapshotMount — point de montage snapshot en lecture-seule ExoFS (no_std).

use alloc::collections::BTreeMap;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{BlobId, FsError};
use super::snapshot::{Snapshot, SnapshotId, flags};
use super::snapshot_list::SNAPSHOT_LIST;

pub static SNAPSHOT_MOUNT: SnapshotMount = SnapshotMount::new_const();

/// Point de montage actif.
#[derive(Clone, Debug)]
pub struct MountPoint {
    pub snap_id:    SnapshotId,
    pub root_blob:  BlobId,
    pub readonly:   bool,
    pub mount_tick: u64,
}

pub struct SnapshotMount {
    mounts: SpinLock<BTreeMap<u64, MountPoint>>,
    next_mount_id: core::sync::atomic::AtomicU64,
}

impl SnapshotMount {
    pub const fn new_const() -> Self {
        Self {
            mounts: SpinLock::new(BTreeMap::new()),
            next_mount_id: core::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Monte un snapshot; retourne un mount_id.
    pub fn mount(&self, snap_id: SnapshotId) -> Result<u64, FsError> {
        let snap = SNAPSHOT_LIST.get(snap_id).ok_or(FsError::NotFound)?;
        let readonly = snap.is_readonly() || snap.is_protected();
        let mount_id = self.next_mount_id.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        let mp = MountPoint {
            snap_id,
            root_blob:  snap.root_blob,
            readonly,
            mount_tick: crate::arch::time::read_ticks(),
        };
        let mut mounts = self.mounts.lock();
        mounts.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        mounts.insert(mount_id, mp);
        Ok(mount_id)
    }

    pub fn umount(&self, mount_id: u64) -> bool {
        self.mounts.lock().remove(&mount_id).is_some()
    }

    pub fn get_root(&self, mount_id: u64) -> Option<BlobId> {
        self.mounts.lock().get(&mount_id).map(|mp| mp.root_blob)
    }

    pub fn is_readonly(&self, mount_id: u64) -> bool {
        self.mounts.lock().get(&mount_id).map_or(true, |mp| mp.readonly)
    }

    pub fn n_active_mounts(&self) -> usize {
        self.mounts.lock().len()
    }
}
