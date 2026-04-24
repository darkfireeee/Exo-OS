//! snapshot_mount.rs — Montage de snapshots en lecture seule
//!
//! Gère le registre des snapshots montés, les points de montage virtuels
//! et s'assure qu'un snapshot monté ne peut pas être supprimé ni modifié.
//!
//! Règles spec :
//!   OOM-02   : try_reserve avant chaque push
//!   ARITH-02 : checked_add pour compteurs

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

/// Point de montage d'un snapshot
#[derive(Debug, Clone)]
pub struct MountPoint {
    /// Identifiant de montage
    pub mount_id: MountId,
    /// Snapshot monté
    pub snap_id: SnapshotId,
    /// Chemin virtuel (UTF-8, null-padded)
    pub path: [u8; 256],
    /// Timestamp de montage (ticks)
    pub mounted_at: u64,
    /// Nombre d'ouvertures actives sur ce point de montage
    pub open_count: u64,
    /// Options de montage
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
// Options de montage
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct MountOptions {
    /// Montage en lecture seule (toujours true pour les snapshots)
    pub readonly: bool,
    /// Montage non-bloquant : n'attend pas les I/O
    pub nonblock: bool,
    /// Montage avec strict verify (vérifie chaque accès)
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

    // ── Montage ─────────────────────────────────────────────────────

    /// Monte un snapshot sur le chemin virtuel `path`
    pub fn mount(
        &self,
        snap_id: SnapshotId,
        path: &[u8],
        opts: MountOptions,
        now: u64,
    ) -> ExofsResult<MountId> {
        // Le snapshot doit exister
        let snap = SNAPSHOT_LIST.get(snap_id)?;

        // Un snapshot en cours de restauration ne peut pas être monté
        if snap.is_restoring() {
            return Err(ExofsError::InvalidState);
        }

        // Construire le point de montage
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

        // Marquer le snapshot comme monté
        SNAPSHOT_LIST.set_flags(snap_id, flags::MOUNTED | flags::READONLY)?;
        self.n_mounts.fetch_add(1, Ordering::AcqRel);
        Ok(mount_id)
    }

    // ── Démontage ───────────────────────────────────────────────────

    /// Démonte un point de montage
    pub fn umount(&self, mount_id: MountId) -> ExofsResult<()> {
        let mut guard = self.mounts.lock();
        let mp = guard.get(&mount_id.0).ok_or(ExofsError::NotFound)?;
        if mp.is_busy() {
            return Err(ExofsError::InvalidState); // Fichiers ouverts
        }
        let snap_id = mp.snap_id;
        guard.remove(&mount_id.0);
        drop(guard);

        self.n_mounts.fetch_sub(1, Ordering::AcqRel);

        // Retirer le flag MOUNTED si plus aucun montage sur ce snapshot
        if !self.is_snap_mounted(snap_id) {
            let _ = SNAPSHOT_LIST.clear_flags(snap_id, flags::MOUNTED);
        }
        Ok(())
    }

    /// Force le démontage (même si des fichiers sont ouverts)
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

    // ── Gestion des ouvertures ───────────────────────────────────────

    /// Incrémente le compteur d'ouvertures (ARITH-02 : checked)
    pub fn open(&self, mount_id: MountId) -> ExofsResult<()> {
        let mut guard = self.mounts.lock();
        let mp = guard.get_mut(&mount_id.0).ok_or(ExofsError::NotFound)?;
        mp.open_count = mp.open_count.checked_add(1).ok_or(ExofsError::Overflow)?;
        Ok(())
    }

    /// Décrémente le compteur d'ouvertures
    pub fn close(&self, mount_id: MountId) -> ExofsResult<()> {
        let mut guard = self.mounts.lock();
        let mp = guard.get_mut(&mount_id.0).ok_or(ExofsError::NotFound)?;
        mp.open_count = mp.open_count.saturating_sub(1);
        Ok(())
    }

    // ── Requêtes ────────────────────────────────────────────────────

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

    // ── Statistiques ────────────────────────────────────────────────

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

    // ── Nettoyage ────────────────────────────────────────────────────

    pub fn clear(&self) {
        let mut guard = self.mounts.lock();
        guard.clear();
        self.n_mounts.store(0, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────
// Statistiques
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
mod tests {
    use super::super::snapshot::{make_snapshot_name, Snapshot};
    use super::super::snapshot_list::SnapshotList;
    use super::*;
    use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, SnapshotId};

    fn push_snap(list: &SnapshotList, id: u64) {
        list.register(Snapshot {
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
        .unwrap();
    }

    #[test]
    fn mount_and_umount() {
        let list = SnapshotList::new_const();
        push_snap(&list, 1);
        let reg = SnapshotMountRegistry::new_const();
        let mid = reg
            .mount(SnapshotId(1), b"/snap/1", MountOptions::default(), 0)
            .unwrap();
        assert!(reg.is_snap_mounted(SnapshotId(1)));
        reg.umount(mid).unwrap();
        assert!(!reg.is_snap_mounted(SnapshotId(1)));
    }

    #[test]
    fn busy_mount_blocks_umount() {
        let list = SnapshotList::new_const();
        push_snap(&list, 2);
        let reg = SnapshotMountRegistry::new_const();
        let mid = reg
            .mount(SnapshotId(2), b"/snap/2", MountOptions::default(), 0)
            .unwrap();
        reg.open(mid).unwrap();
        let err = reg.umount(mid);
        assert!(matches!(err, Err(ExofsError::InvalidState)));
        reg.close(mid).unwrap();
        reg.umount(mid).unwrap();
    }

    #[test]
    fn force_umount_ignores_busy() {
        let list = SnapshotList::new_const();
        push_snap(&list, 3);
        let reg = SnapshotMountRegistry::new_const();
        let mid = reg
            .mount(SnapshotId(3), b"/snap/3", MountOptions::default(), 0)
            .unwrap();
        reg.open(mid).unwrap();
        reg.force_umount(mid).unwrap();
        assert!(!reg.is_snap_mounted(SnapshotId(3)));
    }

    #[test]
    fn find_by_path() {
        let list = SnapshotList::new_const();
        push_snap(&list, 4);
        let reg = SnapshotMountRegistry::new_const();
        reg.mount(SnapshotId(4), b"/mnt/snap4", MountOptions::default(), 0)
            .unwrap();
        let mp = reg.find_by_path(b"/mnt/snap4");
        assert!(mp.is_some());
        assert_eq!(mp.unwrap().snap_id, SnapshotId(4));
    }

    #[test]
    fn open_count_overflow_safe() {
        let list = SnapshotList::new_const();
        push_snap(&list, 5);
        let reg = SnapshotMountRegistry::new_const();
        let mid = reg
            .mount(SnapshotId(5), b"/snap/5", MountOptions::default(), 0)
            .unwrap();
        {
            let mut g = reg.mounts.lock();
            g.get_mut(&mid.0).unwrap().open_count = u64::MAX;
        }
        let err = reg.open(mid);
        assert!(matches!(err, Err(ExofsError::Overflow)));
    }
}
