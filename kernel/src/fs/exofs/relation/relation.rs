//! Relation — définition et identifiant d'une relation ExoFS (no_std).

use crate::fs::exofs::core::BlobId;
use super::relation_type::RelationType;

/// Identifiant unique d'une relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RelationId(pub u64);

/// Représentation on-disk d'une relation.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RelationOnDisk {
    pub id:         u64,
    pub from_blob:  [u8; 32],
    pub to_blob:    [u8; 32],
    pub kind:       u8,
    pub weight:     u32,
    pub created_at: u64,
    pub _pad:       [u8; 3],
}

const _: () = assert!(core::mem::size_of::<RelationOnDisk>() == 88);

/// Relation en mémoire.
#[derive(Clone, Debug)]
pub struct Relation {
    pub id:         RelationId,
    pub from:       BlobId,
    pub to:         BlobId,
    pub rel_type:   RelationType,
    pub created_at: u64,
}

impl Relation {
    pub fn new(
        id:         RelationId,
        from:       BlobId,
        to:         BlobId,
        rel_type:   RelationType,
        created_at: u64,
    ) -> Self {
        Self { id, from, to, rel_type, created_at }
    }

    pub fn to_on_disk(&self) -> RelationOnDisk {
        RelationOnDisk {
            id:         self.id.0,
            from_blob:  self.from.as_bytes(),
            to_blob:    self.to.as_bytes(),
            kind:       self.rel_type.kind as u8,
            weight:     self.rel_type.weight.0,
            created_at: self.created_at,
            _pad:       [0; 3],
        }
    }

    pub fn from_on_disk(d: &RelationOnDisk) -> Self {
        use super::relation_type::{RelationKind, RelationWeight, RelationType};
        let kind = match d.kind {
            0x01 => RelationKind::Parent,
            0x02 => RelationKind::Child,
            0x03 => RelationKind::Symlink,
            0x04 => RelationKind::HardLink,
            0x05 => RelationKind::Refcount,
            0x06 => RelationKind::Snapshot,
            0x07 => RelationKind::SnapshotBase,
            0x08 => RelationKind::Dedup,
            0x09 => RelationKind::Clone,
            _    => RelationKind::CrossRef,
        };
        Self {
            id:         RelationId(d.id),
            from:       BlobId::from_raw(d.from_blob),
            to:         BlobId::from_raw(d.to_blob),
            rel_type:   RelationType::with_weight(kind, d.weight),
            created_at: d.created_at,
        }
    }
}
