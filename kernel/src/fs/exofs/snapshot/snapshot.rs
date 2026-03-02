//! Snapshot — structure et identifiant d'un snapshot ExoFS (no_std).

use crate::fs::exofs::core::{BlobId, EpochId};

pub const SNAPSHOT_MAGIC: u64 = 0x534E4150_53484F54; // "SNAPSHO T"

/// Identifiant unique d'un snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SnapshotId(pub u64);

/// Flags d'un snapshot.
pub mod flags {
    pub const READONLY:   u32 = 1 << 0;
    pub const PROTECTED:  u32 = 1 << 1;
    pub const STREAMING:  u32 = 1 << 2;
    pub const QUOTA_SET:  u32 = 1 << 3;
}

/// En-tête on-disk d'un snapshot.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SnapshotHeader {
    pub magic:       u64,   // SNAPSHOT_MAGIC
    pub id:          u64,
    pub epoch_id:    u64,
    pub parent_id:   u64,   // 0 = pas de parent.
    pub root_blob:   [u8; 32],
    pub created_at:  u64,
    pub n_blobs:     u64,
    pub total_bytes: u64,
    pub flags:       u32,
    pub name_len:    u16,
    pub _pad:        [u8; 2],
    pub name:        [u8; 64],
    pub checksum:    u64,   // XXHash64 de tous les champs précédents.
}

const _: () = assert!(core::mem::size_of::<SnapshotHeader>() == 168);

/// Snapshot en mémoire.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id:          SnapshotId,
    pub epoch_id:    EpochId,
    pub parent_id:   Option<SnapshotId>,
    pub root_blob:   BlobId,
    pub created_at:  u64,
    pub n_blobs:     u64,
    pub total_bytes: u64,
    pub flags:       u32,
    pub name:        [u8; 64],
}

impl Snapshot {
    pub fn is_readonly(&self)  -> bool { self.flags & flags::READONLY  != 0 }
    pub fn is_protected(&self) -> bool { self.flags & flags::PROTECTED != 0 }

    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(64);
        core::str::from_utf8(&self.name[..end]).unwrap_or("<invalid>")
    }

    pub fn to_header(&self, checksum: u64) -> SnapshotHeader {
        SnapshotHeader {
            magic:       SNAPSHOT_MAGIC,
            id:          self.id.0,
            epoch_id:    self.epoch_id.0,
            parent_id:   self.parent_id.map_or(0, |p| p.0),
            root_blob:   self.root_blob.as_bytes(),
            created_at:  self.created_at,
            n_blobs:     self.n_blobs,
            total_bytes: self.total_bytes,
            flags:       self.flags,
            name_len:    self.name.iter().position(|&b| b == 0).unwrap_or(64) as u16,
            _pad:        [0; 2],
            name:        self.name,
            checksum,
        }
    }

    /// Désérialise un header on-disk (RÈGLE 8 : magic en premier).
    pub fn from_header(h: &SnapshotHeader) -> Result<Self, crate::fs::exofs::core::FsError> {
        if h.magic != SNAPSHOT_MAGIC {
            return Err(crate::fs::exofs::core::FsError::InvalidMagic);
        }
        Ok(Self {
            id:          SnapshotId(h.id),
            epoch_id:    EpochId(h.epoch_id),
            parent_id:   if h.parent_id == 0 { None } else { Some(SnapshotId(h.parent_id)) },
            root_blob:   BlobId::from_raw(h.root_blob),
            created_at:  h.created_at,
            n_blobs:     h.n_blobs,
            total_bytes: h.total_bytes,
            flags:       h.flags,
            name:        h.name,
        })
    }
}
