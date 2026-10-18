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

pub static VFS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Enregistre ExoFS comme opérateur VFS actif.
pub fn register_exofs_vfs_ops() -> ExofsResult<()> {
    if VFS_REGISTERED
        .compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)
        .is_err()
    {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    INODE_EMULATION.ensure_root()?;
    DIRECTORY_REGISTRY.ensure_dir(VFS_ROOT_INO)?;
    let _ = load_vfs_namespace_if_disk()?;
    Ok(())
}

/// Retourne vrai si le VFS ExoFS est enregistré.
pub fn vfs_is_registered() -> bool {
    VFS_REGISTERED.load(Ordering::Acquire)
}

/// Retourne le numéro d'inode de la racine du FS.
pub fn root_inode() -> ObjectIno {
    VFS_ROOT_INO
}

pub fn vfs_namespace_blob_id() -> BlobId {
    BlobId::from_bytes_blake3(VFS_NAMESPACE_BLOB_TAG)
}

pub fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn read_u16(buf: &[u8], off: usize) -> ExofsResult<u16> {
    if off.saturating_add(2) > buf.len() {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(u16::from_le_bytes([buf[off], buf[off + 1]]))
}

pub fn read_u32(buf: &[u8], off: usize) -> ExofsResult<u32> {
    if off.saturating_add(4) > buf.len() {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(u32::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
    ]))
}

pub fn read_u64(buf: &[u8], off: usize) -> ExofsResult<u64> {
    if off.saturating_add(8) > buf.len() {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ]))
}

pub fn serialize_vfs_namespace() -> ExofsResult<Vec<u8>> {
    let (inodes, next_ino) = INODE_EMULATION.snapshot_entries()?;
    let records = DIRECTORY_REGISTRY.snapshot_records()?;
    let inode_bytes = inodes
        .len()
        .checked_mul(core::mem::size_of::<InodeEntry>())
        .ok_or(ExofsError::OffsetOverflow)?;
    let mut dir_bytes = 0usize;
    let mut i = 0usize;
    while i < records.len() {
        let name_len = records[i].1.name.len();
        if name_len == 0 || name_len > VFS_NAME_MAX {
            return Err(ExofsError::InvalidPathComponent);
        }
        dir_bytes = dir_bytes
            .checked_add(VFS_NAMESPACE_DIRENT_HEADER_SIZE)
            .and_then(|n| n.checked_add(name_len))
            .ok_or(ExofsError::OffsetOverflow)?;
        i = i.wrapping_add(1);
    }
    let total = VFS_NAMESPACE_HEADER_SIZE
        .checked_add(inode_bytes)
        .and_then(|n| n.checked_add(dir_bytes))
        .ok_or(ExofsError::OffsetOverflow)?;
    let mut out = Vec::new();
    out.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;

    push_u32(&mut out, VFS_NAMESPACE_MAGIC);
    push_u16(&mut out, VFS_NAMESPACE_VERSION);
    push_u16(&mut out, 0);
    push_u64(&mut out, next_ino);
    push_u32(&mut out, inodes.len() as u32);
    push_u32(&mut out, records.len() as u32);
    push_u64(&mut out, 0);

    let mut inode_idx = 0usize;
    while inode_idx < inodes.len() {
        out.extend_from_slice(&encode_inode_entry(&inodes[inode_idx]));
        inode_idx = inode_idx.wrapping_add(1);
    }

    let mut record_idx = 0usize;
    while record_idx < records.len() {
        let (parent, record) = &records[record_idx];
        push_u64(&mut out, *parent);
        push_u64(&mut out, record.ino);
        out.push(record.kind);
        out.push(record.name.len() as u8);
        push_u16(&mut out, 0);
        push_u32(&mut out, 0);
        out.extend_from_slice(&record.name);
        record_idx = record_idx.wrapping_add(1);
    }
    Ok(out)
}

pub fn deserialize_vfs_namespace(
    buf: &[u8],
) -> ExofsResult<(Vec<InodeEntry>, ObjectIno, Vec<(ObjectIno, DirRecord)>)> {
    if buf.len() < VFS_NAMESPACE_HEADER_SIZE {
        return Err(ExofsError::CorruptedStructure);
    }
    if read_u32(buf, 0)? != VFS_NAMESPACE_MAGIC {
        return Err(ExofsError::InvalidMagic);
    }
    if read_u16(buf, 4)? != VFS_NAMESPACE_VERSION {
        return Err(ExofsError::IncompatibleVersion);
    }
    let next_ino = read_u64(buf, 8)?;
    let inode_count = read_u32(buf, 16)? as usize;
    let record_count = read_u32(buf, 20)? as usize;
    let inode_bytes = inode_count
        .checked_mul(core::mem::size_of::<InodeEntry>())
        .ok_or(ExofsError::OffsetOverflow)?;
    let mut off = VFS_NAMESPACE_HEADER_SIZE;
    if off.saturating_add(inode_bytes) > buf.len() {
        return Err(ExofsError::CorruptedStructure);
    }

    let mut inodes = Vec::new();
    inodes
        .try_reserve(inode_count)
        .map_err(|_| ExofsError::NoMemory)?;
    let mut has_root = false;
    let mut i = 0usize;
    while i < inode_count {
        let end = off.saturating_add(core::mem::size_of::<InodeEntry>());
        let entry = decode_inode_entry(&buf[off..end]).ok_or(ExofsError::CorruptedStructure)?;
        if entry.ino == VFS_ROOT_INO && entry.flags & inode_flags::DIRECTORY != 0 {
            has_root = true;
        }
        inodes.push(entry);
        off = end;
        i = i.wrapping_add(1);
    }
    if !has_root {
        return Err(ExofsError::CorruptedStructure);
    }

    let mut records = Vec::new();
    records
        .try_reserve(record_count)
        .map_err(|_| ExofsError::NoMemory)?;
    let mut j = 0usize;
    while j < record_count {
        if off.saturating_add(VFS_NAMESPACE_DIRENT_HEADER_SIZE) > buf.len() {
            return Err(ExofsError::CorruptedStructure);
        }
        let parent = read_u64(buf, off)?;
        let ino = read_u64(buf, off + 8)?;
        let kind = buf[off + 16];
        let name_len = buf[off + 17] as usize;
        off = off.saturating_add(VFS_NAMESPACE_DIRENT_HEADER_SIZE);
        if name_len == 0 || name_len > VFS_NAME_MAX || off.saturating_add(name_len) > buf.len() {
            return Err(ExofsError::CorruptedStructure);
        }
        validate_name(&buf[off..off + name_len])?;
        let mut name = Vec::new();
        name.try_reserve(name_len)
            .map_err(|_| ExofsError::NoMemory)?;
        name.extend_from_slice(&buf[off..off + name_len]);
        records.push((parent, DirRecord { ino, kind, name }));
        off = off.saturating_add(name_len);
        j = j.wrapping_add(1);
    }
    Ok((inodes, next_ino, records))
}

pub fn load_vfs_namespace_if_disk() -> ExofsResult<bool> {
    let Some(data) = object_store::load_blob_data_if_available(&vfs_namespace_blob_id())? else {
        return Ok(false);
    };
    let (inodes, next_ino, records) = deserialize_vfs_namespace(&data)?;
    INODE_EMULATION.replace_entries(&inodes, next_ino)?;
    DIRECTORY_REGISTRY.replace_records(&records)?;
    Ok(true)
}

pub fn persist_vfs_namespace_if_disk() -> ExofsResult<bool> {
    let data = serialize_vfs_namespace()?;
    object_store::persist_blob_data_if_disk(vfs_namespace_blob_id(), &data, true)
}

