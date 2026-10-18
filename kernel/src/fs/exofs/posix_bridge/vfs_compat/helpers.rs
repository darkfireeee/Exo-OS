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
// Helpers internes
// ─────────────────────────────────────────────────────────────────────────────

pub fn blob_id_for_object(object_id: u64) -> BlobId {
    BlobId::from_bytes_blake3(&object_id.to_le_bytes())
}

pub fn ensure_inode_blob_cached(entry: &InodeEntry) -> ExofsResult<BlobId> {
    let blob_id = blob_id_for_object(entry.object_id);
    if BLOB_CACHE.contains(&blob_id) {
        return Ok(blob_id);
    }
    if let Some(data) = object_store::load_blob_data_if_available(&blob_id)? {
        BLOB_CACHE.insert(blob_id, data)?;
    } else {
        BLOB_CACHE.insert(blob_id, Vec::new())?;
    }
    Ok(blob_id)
}

pub fn resize_inode_data(entry: &InodeEntry, new_len: usize) -> ExofsResult<()> {
    let blob_id = ensure_inode_blob_cached(entry)?;
    reject_if_immutable(blob_id)?;
    let current_len = BLOB_CACHE.len(&blob_id).unwrap_or(0);
    if new_len > current_len {
        check_quota(entry.uid, (new_len - current_len) as u64, 0)?;
    }
    BLOB_CACHE.resize(blob_id, new_len)?;
    persist_cached_blob_if_disk(blob_id)?;
    if new_len > current_len {
        quota_add_usage(entry.uid, (new_len - current_len) as u64, 0)?;
    } else if current_len > new_len {
        quota_sub_usage(entry.uid, (current_len - new_len) as u64, 0)?;
    }
    Ok(())
}

/// Convertit un InodeEntry en VfsInode.
pub fn entry_to_vfs_inode(e: &InodeEntry) -> VfsInode {
    let mode = if e.flags & inode_flags::DIRECTORY != 0 {
        file_mode::DEFAULT_DIR
    } else if e.flags & inode_flags::SYMLINK != 0 {
        file_mode::S_IFLNK | 0o777
    } else {
        file_mode::DEFAULT_FILE
    };
    VfsInode {
        ino: e.ino,
        size: e.size,
        mode,
        uid: e.uid as u32,
        link_count: e.link_count,
        kind: inode_kind(e.flags),
        _pad: [0; 3],
    }
}

pub fn inode_kind(flags: u32) -> u8 {
    if flags & inode_flags::DIRECTORY != 0 {
        4
    } else if flags & inode_flags::SYMLINK != 0 {
        10
    } else {
        8
    }
}

/// Construit un VfsDirent depuis un ino et un nom.
pub fn make_dirent(ino: ObjectIno, name: &[u8], kind: u8) -> VfsDirent {
    let mut d = VfsDirent {
        ino,
        kind,
        name_len: name.len().min(VFS_NAME_MAX) as u8,
        _pad: [0; 6],
        name: [0; VFS_NAME_MAX],
    };
    let mut i = 0usize;
    while i < d.name_len as usize {
        d.name[i] = name[i];
        i = i.wrapping_add(1);
    }
    d
}

/// Valide un composant de nom (pas de '/', '\0', pas ".." admis ici).
pub fn validate_name(name: &[u8]) -> ExofsResult<()> {
    let mut i = 0usize;
    while i < name.len() {
        if name[i] == b'/' || name[i] == 0 {
            return Err(ExofsError::InvalidPathComponent);
        }
        i = i.wrapping_add(1);
    }
    Ok(())
}

/// Hash de nom déterministe : FNV-1a 64-bit sur (parent_ino, name).
/// ARITH-02 : wrapping_mul/xor.
pub fn hash_name(parent_ino: ObjectIno, name: &[u8]) -> u64 {
    pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET;
    let pid_bytes = parent_ino.to_le_bytes();
    let mut i = 0usize;
    while i < 8 {
        h = h.wrapping_mul(FNV_PRIME) ^ (pid_bytes[i] as u64);
        i = i.wrapping_add(1);
    }
    let mut j = 0usize;
    while j < name.len() {
        h = h.wrapping_mul(FNV_PRIME) ^ (name[j] as u64);
        j = j.wrapping_add(1);
    }
    // Évite oid == 0.
    if h == 0 {
        1
    } else {
        h
    }
}

