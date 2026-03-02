//! SnapshotProtect — protection et déprotection de snapshots ExoFS (no_std).

use crate::fs::exofs::core::FsError;
use super::snapshot::{SnapshotId, flags};
use super::snapshot_list::SNAPSHOT_LIST;

pub struct SnapshotProtect;

impl SnapshotProtect {
    /// Active la protection (flag PROTECTED + READONLY).
    pub fn protect(id: SnapshotId) -> Result<(), FsError> {
        Self::set_flags(id, flags::PROTECTED | flags::READONLY, true)
    }

    /// Désactive la protection (kernel interne seulement).
    pub fn unprotect(id: SnapshotId) -> Result<(), FsError> {
        Self::set_flags(id, flags::PROTECTED, false)
    }

    /// Passe en lecture seule sans protection forte.
    pub fn set_readonly(id: SnapshotId, readonly: bool) -> Result<(), FsError> {
        Self::set_flags(id, flags::READONLY, readonly)
    }

    fn set_flags(id: SnapshotId, mask: u32, set: bool) -> Result<(), FsError> {
        // On doit modifier le snapshot dans le registre.
        // SnapshotList expose une mutation interne via SpinLock.
        let mut list = SNAPSHOT_LIST.snapshots.lock();
        if let Some(s) = list.get_mut(&id.0) {
            if set { s.flags |= mask; } else { s.flags &= !mask; }
            Ok(())
        } else {
            Err(FsError::NotFound)
        }
    }

    pub fn is_protected(id: SnapshotId) -> bool {
        SNAPSHOT_LIST.is_protected(id)
    }
}
