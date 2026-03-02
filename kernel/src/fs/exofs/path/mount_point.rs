// path/mount_point.rs — Table de montage ExoFS (intégration VFS)
// Ring 0, no_std

use crate::fs::exofs::core::{ObjectId, ExofsError};
use crate::scheduler::sync::spinlock::SpinLock;
use alloc::vec::Vec;

/// Entrée de montage
struct MountEntry {
    /// ObjectId du répertoire où le montage a lieu
    mount_point_oid: ObjectId,
    /// Nom du composant (ex: b"proc", b"sys")
    component: [u8; 255],
    component_len: u8,
    /// ObjectId de la racine du système monté
    mounted_root_oid: ObjectId,
}

/// Table de montage globale
pub struct MountTable {
    inner: SpinLock<MountTableInner>,
}

struct MountTableInner {
    entries: Vec<MountEntry>,
}

/// Instance globale
pub static MOUNT_TABLE: MountTable = MountTable {
    inner: SpinLock::new(MountTableInner { entries: Vec::new() }),
};

impl MountTable {
    /// Vérifie si un composant dans un répertoire est un point de montage
    pub fn lookup_mount(&self, dir_oid: ObjectId, component: &[u8]) -> Option<ObjectId> {
        let guard = self.inner.lock();
        for entry in &guard.entries {
            if entry.mount_point_oid.ct_eq(&dir_oid)
                && entry.component_len as usize == component.len()
                && &entry.component[..component.len()] == component
            {
                return Some(entry.mounted_root_oid);
            }
        }
        None
    }

    /// Enregistre un montage
    pub fn register(
        &self,
        dir_oid: ObjectId,
        component: &[u8],
        root_oid: ObjectId,
    ) -> Result<(), ExofsError> {
        if component.len() > 255 {
            return Err(ExofsError::NameTooLong);
        }
        let mut guard = self.inner.lock();
        guard.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;

        let mut comp_arr = [0u8; 255];
        comp_arr[..component.len()].copy_from_slice(component);
        guard.entries.push(MountEntry {
            mount_point_oid: dir_oid,
            component: comp_arr,
            component_len: component.len() as u8,
            mounted_root_oid: root_oid,
        });
        Ok(())
    }

    /// Supprime un montage
    pub fn unregister(&self, dir_oid: ObjectId, component: &[u8]) -> Result<(), ExofsError> {
        let mut guard = self.inner.lock();
        let pos = guard.entries.iter().position(|e| {
            e.mount_point_oid.ct_eq(&dir_oid)
                && &e.component[..e.component_len as usize] == component
        }).ok_or(ExofsError::ObjectNotFound)?;
        guard.entries.remove(pos);
        Ok(())
    }
}
