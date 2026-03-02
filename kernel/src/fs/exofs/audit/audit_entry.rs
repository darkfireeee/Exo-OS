//! AuditEntry — entrée individuelle de journal d'audit ExoFS (no_std).

use crate::fs::exofs::core::BlobId;

/// Opération auditée.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditOp {
    Read         = 0x01,
    Write        = 0x02,
    Create       = 0x03,
    Delete       = 0x04,
    Rename       = 0x05,
    SetMeta      = 0x06,
    SnapshotCreate = 0x07,
    SnapshotDelete = 0x08,
    EpochCommit  = 0x09,
    GcTrigger    = 0x0A,
    Export       = 0x0B,
    Import       = 0x0C,
    CryptoKey    = 0x0D,
}

/// Résultat d'une opération.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditResult {
    Success = 0,
    Denied  = 1,
    Error   = 2,
}

/// Entrée d'audit (taille fixe 64 B).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AuditEntry {
    pub tick:       u64,
    pub actor_uid:  u64,
    pub actor_cap:  u64,
    pub object_id:  u64,
    pub blob_id:    [u8; 32],
    pub op:         AuditOp,
    pub result:     AuditResult,
    pub _pad:       [u8; 6],
}

const _: () = assert!(core::mem::size_of::<AuditEntry>() == 64);
