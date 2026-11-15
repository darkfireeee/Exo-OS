//! snapshot_mount.rs — Read-only mounting of snapshots
//!
//! Manages the registry of mounted snapshots, virtual mount points,
//! and ensures that a mounted snapshot cannot be deleted or modified.
//!
//! Spec rules:
//!   OOM-02   : try_reserve before each push
//!   ARITH-02 : checked_add for counters

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::snapshot::flags;
use super::snapshot_list::SNAPSHOT_LIST;
use crate::fs::exofs::core::{ExofsError, ExofsResult, SnapshotId};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────

pub static SNAPSHOT_MOUNT: SnapshotMountRegistry = SnapshotMountRegistry::new_const();

// ─────────────────────────────────────────────────────────────
// MountId
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MountId(pub u64);

// ─────────────────────────────────────────────────────────────
// MountPoint
// ─────────────────────────────────────────────────────────────

/// Mount point of a snapshot
#[derive(Debug, Clone)]
pub struct MountPoint {
    /// Mount identifier
    pub mount_id: MountId,
    /// Mounted snapshot
    pub snap_id: SnapshotId,
    /// Virtual path (UTF-8, null-padded)
    pub path: [u8; 256],
    /// Mount timestamp (ticks)
    pub mounted_at: u64,
    /// Number of active opens on this mount point
    pub open_count: u64,
    /// Mount options
    pub opts: MountOptions,
}

impl MountPoint {
    pub fn path_str(&self) -> &str {
        let end = self.path.iter().position(|&b| b == 0).unwrap_or(256);
        core::str::from_utf8(&self.path[..end]).unwrap_or("<invalid>")
    }

    pub fn is_busy(&self) -> bool {
        self.open_count > 0
    }
}

// ─────────────────────────────────────────────────────────────
// Mount options
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct MountOptions {
    /// Read-only mount (always true for snapshots)
    pub readonly: bool,
    /// Non-blocking mount: does not wait for I/O
    pub nonblock: bool,
    /// Mount with strict verification (verifies each access)
    pub strict: bool,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            readonly: true,
            nonblock: false,
            strict: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotMountRegistry
// ─────────────────────────────────────────────────────────────

pub struct SnapshotMountRegistry {
    mounts: SpinLock<alloc::collections::BTreeMap<u64, MountPoint>>,
    next_id: AtomicU64,
    n_mounts: AtomicUsize,
}

impl SnapshotMountRegistry {
    pub const fn new_const() -> Self {
        Self {
            mounts: SpinLock::new(alloc::collections::BTreeMap::new()),
            next_id: AtomicU64::new(1),
            n_mounts: AtomicUsize::new(0),
        }
    }

    // ── Mount ────────────────────────────────────────────────

    /// Mounts a snapshot on virtual path `path`
    pub fn mount(
        &self,
        snap_id: SnapshotId,
        path: &[u8],
        opts: MountOptions,
        now: u64,
    ) -> ExofsResult<MountId> {
        // The snapshot must exist
        let snap = SNAPSHOT_LIST.get(snap_id)?;

        // A snapshot currently being restored cannot be mounted
        if snap.is_restoring() {
            return Err(ExofsError::InvalidState);
        }

        // Build mount point
        let mount_id = MountId(self.next_id.fetch_add(1, Ordering::AcqRel));
        let mut path_arr = [0u8; 256];
        let len = path.len().min(256);
        path_arr[..len].copy_from_slice(&path[..len]);

        let mp = MountPoint {
            mount_id,
            snap_id,
            path: path_arr,
            mounted_at: now,
            open_count: 0,
            opts: MountOptions {
                readonly: true,
                ..opts
            },
        };

        let mut guard = self.mounts.lock();
        guard.insert(mount_id.0, mp);
        drop(guard);

        // Mark snapshot as mounted
        SNAPSHOT_LIST.set_flags(snap_id, flags::MOUNTED | flags::READONLY)?;
        self.n_mounts.fetch_add(1, Ordering::AcqRel);
        Ok(mount_id)
    }

    // ── Unmount ──────────────────────────────────────────────

    /// Unmounts a mount point
    pub fn umount(&self, mount_id: MountId) -> ExofsResult<()> {
        let mut guard = self.mounts.lock();
        let mp = guard.get(&mount_id.0).ok_or(ExofsError::NotFound)?;
        if mp.is_busy() {
            return Err(ExofsError::InvalidState); // Open files
        }
        let snap_id = mp.snap_id;
        guard.remove(&mount_id.0);
        drop(guard);

        self.n_mounts.fetch_sub(1, Ordering::AcqRel);

        // Remove MOUNTED flag if no more mounts remain on this snapshot
        if !self.is_snap_mounted(snap_id) {
            let _ = SNAPSHOT_LIST.clear_flags(snap_id, flags::MOUNTED);
        }
        Ok(())
    }

    /// Forces unmount (even if files are open)
    pub fn force_umount(&self, mount_id: MountId) -> ExofsResult<()> {
        let mut guard = self.mounts.lock();
        let mp = guard.remove(&mount_id.0).ok_or(ExofsError::NotFound)?;
        let snap_id = mp.snap_id;
        drop(guard);

        self.n_mounts.fetch_sub(1, Ordering::AcqRel);
        if !self.is_snap_mounted(snap_id) {
            let _ = SNAPSHOT_LIST.clear_flags(snap_id, flags::MOUNTED);
        }
        Ok(())
    }

    // ── Open management ──────────────────────────────────────

    /// Increments the open counter (ARITH-02: checked)
    pub fn open(&self, mount_id: MountId) -> ExofsResult<()> {
        let mut guard = self.mounts.lock();
        let mp = guard.get_mut(&mount_id.0).ok_or(ExofsError::NotFound)?;
        mp.open_count = mp.open_count.checked_add(1).ok_or(ExofsError::Overflow)?;
        Ok(())
    }

    /// Decrements the open counter
    pub fn close(&self, mount_id: MountId) -> ExofsResult<()> {
        let mut guard = self.mounts.lock();
        let mp = guard.get_mut(&mount_id.0).ok_or(ExofsError::NotFound)?;
        mp.open_count = mp.open_count.saturating_sub(1);
        Ok(())
    }

    // ── Queries ──────────────────────────────────────────────

    pub fn get(&self, mount_id: MountId) -> ExofsResult<MountPoint> {
        let guard = self.mounts.lock();
        guard.get(&mount_id.0).cloned().ok_or(ExofsError::NotFound)
    }

    pub fn is_snap_mounted(&self, snap_id: SnapshotId) -> bool {
        let guard = self.mounts.lock();
        guard.values().any(|mp| mp.snap_id == snap_id)
    }

    pub fn mounts_for_snap(&self, snap_id: SnapshotId) -> ExofsResult<Vec<MountId>> {
        let guard = self.mounts.lock();
        let mut out: Vec<MountId> = Vec::new();
        for mp in guard.values() {
            if mp.snap_id == snap_id {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(mp.mount_id);
            }
        }
        Ok(out)
    }

    pub fn find_by_path(&self, path: &[u8]) -> Option<MountPoint> {
        let guard = self.mounts.lock();
        for mp in guard.values() {
            let len = path.len().min(256);
            if &mp.path[..len] == &path[..len] && mp.path[len..].iter().all(|&b| b == 0) {
                return Some(mp.clone());
            }
        }
        None
    }

    pub fn all_mount_ids(&self) -> ExofsResult<Vec<MountId>> {
        let guard = self.mounts.lock();
        let mut out: Vec<MountId> = Vec::new();
        out.try_reserve(guard.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for &k in guard.keys() {
            out.push(MountId(k));
        }
        Ok(out)
    }

    // ── Statistics ───────────────────────────────────────────

    pub fn n_mounts(&self) -> usize {
        self.n_mounts.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> MountStats {
        let guard = self.mounts.lock();
        let mut n_busy: usize = 0;
        let mut total_opens: u64 = 0;
        for mp in guard.values() {
            if mp.is_busy() {
                n_busy += 1;
            }
            total_opens = total_opens.saturating_add(mp.open_count);
        }
        MountStats {
            n_mounts: guard.len(),
            n_busy,
            total_opens,
        }
    }

    // ── Cleanup ──────────────────────────────────────────────

    pub fn clear(&self) {
        let mut guard = self.mounts.lock();
        guard.clear();
        self.n_mounts.store(0, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct MountStats {
    pub n_mounts: usize,
    pub n_busy: usize,
    pub total_opens: u64,
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::reset_for_test;
    use super::super::snapshot::{make_snapshot_name, Snapshot};
    use super::super::snapshot_list::{SnapshotList, SNAPSHOT_LIST};
    use super::*;
    use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, SnapshotId};

    fn push_snap(_list: &SnapshotList, id: u64) {
        SNAPSHOT_LIST
            .register(Snapshot {
                id: SnapshotId(id),
                epoch_id: EpochId(1),
                parent_id: None,
                root_blob: BlobId([0u8; 32]),
                created_at: 0,
                n_blobs: 0,
                total_bytes: 0,
                flags: 0,
                blob_catalog_offset: DiskOffset(0),
                blob_catalog_size: 0,
                name: make_snapshot_name(b"m-test"),
            })
            .test_unwrap();
    }

    #[test]
    fn mount_and_umount() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 1);
        let reg = SnapshotMountRegistry::new_const();
        let mid = reg
            .mount(SnapshotId(1), b"/snap/1", MountOptions::default(), 0)
            .test_unwrap();
        assert!(reg.is_snap_mounted(SnapshotId(1)));
        reg.umount(mid).test_unwrap();
        assert!(!reg.is_snap_mounted(SnapshotId(1)));
    }

    #[test]
    fn busy_mount_blocks_umount() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 2);
        let reg = SnapshotMountRegistry::new_const();
        let mid = reg
            .mount(SnapshotId(2), b"/snap/2", MountOptions::default(), 0)
            .test_unwrap();
        reg.open(mid).test_unwrap();
        let err = reg.umount(mid);
        assert!(matches!(err, Err(ExofsError::InvalidState)));
        reg.close(mid).test_unwrap();
        reg.umount(mid).test_unwrap();
    }

    #[test]
    fn force_umount_ignores_busy() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 3);
        let reg = SnapshotMountRegistry::new_const();
        let mid = reg
            .mount(SnapshotId(3), b"/snap/3", MountOptions::default(), 0)
            .test_unwrap();
        reg.open(mid).test_unwrap();
        reg.force_umount(mid).test_unwrap();
        assert!(!reg.is_snap_mounted(SnapshotId(3)));
    }

    #[test]
    fn find_by_path() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 4);
        let reg = SnapshotMountRegistry::new_const();
        reg.mount(SnapshotId(4), b"/mnt/snap4", MountOptions::default(), 0)
            .test_unwrap();
        let mp = reg.find_by_path(b"/mnt/snap4");
        assert!(mp.is_some());
        assert_eq!(mp.test_unwrap().snap_id, SnapshotId(4));
    }

    #[test]
    fn open_count_overflow_safe() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 5);
        let reg = SnapshotMountRegistry::new_const();
        let mid = reg
            .mount(SnapshotId(5), b"/snap/5", MountOptions::default(), 0)
            .test_unwrap();
        {
            let mut g = reg.mounts.lock();
            g.get_mut(&mid.0).test_unwrap().open_count = u64::MAX;
        }
        let err = reg.open(mid);
        assert!(matches!(err, Err(ExofsError::Overflow)));
    }
}
