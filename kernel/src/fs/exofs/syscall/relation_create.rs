//! SYS_EXOFS_RELATION_CREATE (512) — création de relation via syscall (no_std).
//! RÈGLE 9 : copy_from_user() pour tous les pointeurs userspace.

use crate::fs::exofs::core::{BlobId, FsError};
use crate::fs::exofs::relation::relation_batch::{RelationBatch, BatchOp};
use crate::fs::exofs::relation::relation_type::{RelationKind, RelationType};
use super::validation::copy_struct_from_user;

/// Arguments userspace pour SYS_EXOFS_RELATION_CREATE.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SysRelationCreateArgs {
    pub from_blob: [u8; 32],
    pub to_blob:   [u8; 32],
    pub kind:      u8,
    pub weight:    u32,
    pub _pad:      [u8; 3],
    pub out_id:    u64,   // *mut u64 — RelationId résultant.
}

const _: () = assert!(core::mem::size_of::<SysRelationCreateArgs>() == 80);

pub fn sys_relation_create(args_ptr: u64, _uid: u64) -> i64 {
    let args: SysRelationCreateArgs = match copy_struct_from_user(args_ptr) {
        Ok(a)  => a,
        Err(_) => return -14,
    };

    let from = BlobId::from_raw(args.from_blob);
    let to   = BlobId::from_raw(args.to_blob);

    let kind = match args.kind {
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

    let rel_type = RelationType::with_weight(kind, args.weight);
    let mut batch = RelationBatch::new();
    if batch.add_insert(from, to, rel_type).is_err() { return -12; }

    let result = batch.commit();
    if result.failed > 0 { return -5; }

    0
}
