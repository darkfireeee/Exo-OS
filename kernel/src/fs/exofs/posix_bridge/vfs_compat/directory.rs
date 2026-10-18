#![allow(unused_imports, dead_code)]
use alloc::collections::btree_map::Entry;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::posix_bridge::inode_emulation::{
    decode_inode_entry, encode_inode_entry, inode_flags, InodeEntry, ObjectIno, INODE_EMULATION,
};
use crate::fs::exofs::syscall::object_store;
use crate::fs::exofs::syscall::object_write::{persist_cached_blob_if_disk, reject_if_immutable};
use crate::fs::exofs::syscall::quota_query::{check_quota, quota_add_usage, quota_sub_usage};

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Registre VFS
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct DirRecord {
    pub ino: ObjectIno,
    pub kind: u8,
    pub name: Vec<u8>,
}

pub struct DirectoryRegistry {
    pub entries: UnsafeCell<BTreeMap<ObjectIno, Vec<DirRecord>>>,
    pub spinlock: AtomicU64,
}

unsafe impl Sync for DirectoryRegistry {}
unsafe impl Send for DirectoryRegistry {}

impl DirectoryRegistry {
    pub const fn new() -> Self {
        Self {
            entries: UnsafeCell::new(BTreeMap::new()),
            spinlock: AtomicU64::new(0),
        }
    }

    pub fn lock_acquire(&self) {
        while self
            .spinlock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    pub fn lock_release(&self) {
        self.spinlock.store(0, Ordering::Release);
    }

    pub fn map(&self) -> &mut BTreeMap<ObjectIno, Vec<DirRecord>> {
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        unsafe { &mut *self.entries.get() }
    }

    pub fn ensure_dir(&self, ino: ObjectIno) -> ExofsResult<()> {
        self.lock_acquire();
        let map = self.map();
        if let Entry::Vacant(entry) = map.entry(ino) {
            entry.insert(Vec::new());
        }
        self.lock_release();
        Ok(())
    }

    pub fn lookup(&self, parent_ino: ObjectIno, name: &[u8]) -> Option<DirRecord> {
        self.lock_acquire();
        let result = self
            .map()
            .get(&parent_ino)
            .and_then(|records| records.iter().find(|record| record.name == name))
            .cloned();
        self.lock_release();
        result
    }

    pub fn insert(
        &self,
        parent_ino: ObjectIno,
        ino: ObjectIno,
        name: &[u8],
        kind: u8,
    ) -> ExofsResult<()> {
        let mut name_vec = Vec::new();
        name_vec
            .try_reserve(name.len())
            .map_err(|_| ExofsError::NoMemory)?;
        name_vec.extend_from_slice(name);

        self.lock_acquire();
        let map = self.map();
        if let Entry::Vacant(entry) = map.entry(parent_ino) {
            entry.insert(Vec::new());
        }
        let records = match map.get_mut(&parent_ino) {
            Some(records) => records,
            None => {
                self.lock_release();
                return Err(ExofsError::InternalError);
            }
        };
        if records.iter().any(|record| record.name == name) {
            self.lock_release();
            return Err(ExofsError::ObjectAlreadyExists);
        }
        if records.try_reserve(1).is_err() {
            self.lock_release();
            return Err(ExofsError::NoMemory);
        }
        records.push(DirRecord {
            ino,
            kind,
            name: name_vec,
        });
        self.lock_release();

        if kind == 4 {
            self.ensure_dir(ino)?;
        }
        Ok(())
    }

    pub fn list(&self, parent_ino: ObjectIno) -> ExofsResult<Vec<DirRecord>> {
        self.lock_acquire();
        let records = match self.map().get(&parent_ino).cloned() {
            Some(records) => records,
            None => {
                self.lock_release();
                return Err(ExofsError::ObjectNotFound);
            }
        };
        self.lock_release();
        Ok(records)
    }

    pub fn has_children(&self, parent_ino: ObjectIno) -> bool {
        self.lock_acquire();
        let result = self
            .map()
            .get(&parent_ino)
            .map(|records| !records.is_empty())
            .unwrap_or(false);
        self.lock_release();
        result
    }

    pub fn remove(&self, parent_ino: ObjectIno, name: &[u8]) -> Option<DirRecord> {
        self.lock_acquire();
        let map = self.map();
        let result = map.get_mut(&parent_ino).and_then(|records| {
            records
                .iter()
                .position(|record| record.name == name)
                .map(|idx| records.swap_remove(idx))
        });
        self.lock_release();
        result
    }

    pub fn rename(
        &self,
        old_parent: ObjectIno,
        old_name: &[u8],
        new_parent: ObjectIno,
        new_name: &[u8],
    ) -> ExofsResult<DirRecord> {
        let mut new_name_vec = Vec::new();
        new_name_vec
            .try_reserve(new_name.len())
            .map_err(|_| ExofsError::NoMemory)?;
        new_name_vec.extend_from_slice(new_name);

        self.lock_acquire();
        let map = self.map();
        if map
            .get(&new_parent)
            .map(|records| records.iter().any(|record| record.name == new_name))
            .unwrap_or(false)
        {
            self.lock_release();
            return Err(ExofsError::ObjectAlreadyExists);
        }
        let record = {
            let old_records = match map.get_mut(&old_parent) {
                Some(records) => records,
                None => {
                    self.lock_release();
                    return Err(ExofsError::ObjectNotFound);
                }
            };
            let idx = match old_records
                .iter()
                .position(|record| record.name == old_name)
            {
                Some(idx) => idx,
                None => {
                    self.lock_release();
                    return Err(ExofsError::ObjectNotFound);
                }
            };
            old_records.swap_remove(idx)
        };
        let mut updated = record;
        updated.name = new_name_vec;
        if let Entry::Vacant(entry) = map.entry(new_parent) {
            entry.insert(Vec::new());
        }
        let new_records = match map.get_mut(&new_parent) {
            Some(records) => records,
            None => {
                self.lock_release();
                return Err(ExofsError::InternalError);
            }
        };
        if new_records.try_reserve(1).is_err() {
            self.lock_release();
            return Err(ExofsError::NoMemory);
        }
        new_records.push(updated.clone());
        self.lock_release();
        Ok(updated)
    }

    pub fn remove_dir(&self, ino: ObjectIno) {
        self.lock_acquire();
        self.map().remove(&ino);
        self.lock_release();
    }

    pub fn snapshot_records(&self) -> ExofsResult<Vec<(ObjectIno, DirRecord)>> {
        self.lock_acquire();
        let map = self.map();
        let mut total = 0usize;
        for records in map.values() {
            total = total.saturating_add(records.len());
        }
        let mut out = Vec::new();
        out.try_reserve(total).map_err(|_| {
            self.lock_release();
            ExofsError::NoMemory
        })?;
        for (parent, records) in map.iter() {
            let mut i = 0usize;
            while i < records.len() {
                out.push((*parent, records[i].clone()));
                i = i.wrapping_add(1);
            }
        }
        self.lock_release();
        Ok(out)
    }

    pub fn replace_records(&self, records: &[(ObjectIno, DirRecord)]) -> ExofsResult<()> {
        self.lock_acquire();
        let map = self.map();
        map.clear();
        map.insert(VFS_ROOT_INO, Vec::new());
        let mut i = 0usize;
        while i < records.len() {
            let (parent, record) = &records[i];
            if let Entry::Vacant(entry) = map.entry(*parent) {
                entry.insert(Vec::new());
            }
            let Some(parent_records) = map.get_mut(parent) else {
                self.lock_release();
                return Err(ExofsError::InternalError);
            };
            parent_records.try_reserve(1).map_err(|_| {
                self.lock_release();
                ExofsError::NoMemory
            })?;
            parent_records.push(record.clone());
            if record.kind == inode_kind(inode_flags::DIRECTORY)
                && matches!(map.entry(record.ino), Entry::Vacant(_))
            {
                map.insert(record.ino, Vec::new());
            }
            i = i.wrapping_add(1);
        }
        self.lock_release();
        Ok(())
    }

    #[cfg(test)]
    pub fn reset_all(&self) {
        self.lock_acquire();
        self.map().clear();
        self.map().insert(VFS_ROOT_INO, Vec::new());
        self.lock_release();
    }
}

pub static DIRECTORY_REGISTRY: DirectoryRegistry = DirectoryRegistry::new();
