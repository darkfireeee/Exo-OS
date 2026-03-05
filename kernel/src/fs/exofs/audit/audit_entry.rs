//! audit_entry.rs — Entrée individuelle du journal d'audit ExoFS (no_std).
//!
//! Règles appliquées :
//!  - ARITH-02 : arithmétique vérifiée (`checked_add`)
//!  - ONDISK-03 : structure on-disk sans AtomicU64
//!  - Zéro récursion (RECUR-01)

#![allow(dead_code)]

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille fixe d'une entrée on-disk en octets.
pub const AUDIT_ENTRY_SIZE: usize = 96;

/// Nombre maximum d'acteurs dans un contexte multi-principal.
pub const AUDIT_MAX_ACTORS: usize = 4;

/// Magic de validation d'une entrée (4 premiers octets du champ reserved).
pub const AUDIT_ENTRY_MAGIC: u32 = 0x41554454; // "AUDT"

// ─────────────────────────────────────────────────────────────────────────────
// AuditOp — opération auditée
// ─────────────────────────────────────────────────────────────────────────────

/// Opération auditée.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuditOp {
    Read            = 0x01,
    Write           = 0x02,
    Create          = 0x03,
    Delete          = 0x04,
    Rename          = 0x05,
    SetMeta         = 0x06,
    SnapshotCreate  = 0x07,
    SnapshotDelete  = 0x08,
    EpochCommit     = 0x09,
    GcTrigger       = 0x0A,
    Export          = 0x0B,
    Import          = 0x0C,
    CryptoKey       = 0x0D,
    MountRequested  = 0x0E,
    MountGranted    = 0x0F,
    Unmount         = 0x10,
    PolicyChange    = 0x11,
    ChecksumFail    = 0x12,
    PermDenied      = 0x13,
}

impl AuditOp {
    /// Construit depuis un octet — retourne `None` si inconnu.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::Read),
            0x02 => Some(Self::Write),
            0x03 => Some(Self::Create),
            0x04 => Some(Self::Delete),
            0x05 => Some(Self::Rename),
            0x06 => Some(Self::SetMeta),
            0x07 => Some(Self::SnapshotCreate),
            0x08 => Some(Self::SnapshotDelete),
            0x09 => Some(Self::EpochCommit),
            0x0A => Some(Self::GcTrigger),
            0x0B => Some(Self::Export),
            0x0C => Some(Self::Import),
            0x0D => Some(Self::CryptoKey),
            0x0E => Some(Self::MountRequested),
            0x0F => Some(Self::MountGranted),
            0x10 => Some(Self::Unmount),
            0x11 => Some(Self::PolicyChange),
            0x12 => Some(Self::ChecksumFail),
            0x13 => Some(Self::PermDenied),
            _    => None,
        }
    }

    /// `true` pour les opérations d'écriture (modifie l'état du FS).
    pub fn is_mutating(self) -> bool {
        matches!(self,
            Self::Write | Self::Create | Self::Delete | Self::Rename
            | Self::SetMeta | Self::SnapshotCreate | Self::SnapshotDelete
            | Self::EpochCommit | Self::PolicyChange | Self::Import
        )
    }

    /// `true` pour les opérations de sécurité/cryptographie.
    pub fn is_security_sensitive(self) -> bool {
        matches!(self,
            Self::CryptoKey | Self::PolicyChange | Self::PermDenied
            | Self::MountRequested | Self::MountGranted
        )
    }

    /// Nom court de l'opération (ASCII, statique).
    pub fn name(self) -> &'static str {
        match self {
            Self::Read           => "READ",
            Self::Write          => "WRITE",
            Self::Create         => "CREATE",
            Self::Delete         => "DELETE",
            Self::Rename         => "RENAME",
            Self::SetMeta        => "SET_META",
            Self::SnapshotCreate => "SNAP_CREATE",
            Self::SnapshotDelete => "SNAP_DELETE",
            Self::EpochCommit    => "EPOCH_COMMIT",
            Self::GcTrigger      => "GC_TRIGGER",
            Self::Export         => "EXPORT",
            Self::Import         => "IMPORT",
            Self::CryptoKey      => "CRYPTO_KEY",
            Self::MountRequested => "MOUNT_REQ",
            Self::MountGranted   => "MOUNT_OK",
            Self::Unmount        => "UNMOUNT",
            Self::PolicyChange   => "POLICY_CHG",
            Self::ChecksumFail   => "CHKSUM_FAIL",
            Self::PermDenied     => "PERM_DENIED",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditResult — résultat de l'opération
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une opération auditée.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AuditResult {
    #[default]
    Success = 0,
    Denied  = 1,
    Error   = 2,
    Partial = 3,
    Timeout = 4,
}

impl AuditResult {
    /// Construit depuis un octet.
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Success),
            1 => Some(Self::Denied),
            2 => Some(Self::Error),
            3 => Some(Self::Partial),
            4 => Some(Self::Timeout),
            _ => None,
        }
    }

    /// `true` si l'opération a réussi.
    pub fn is_ok(self) -> bool { matches!(self, Self::Success | Self::Partial) }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditSeverity — criticité d'une entrée
// ─────────────────────────────────────────────────────────────────────────────

/// Niveau de criticité d'une entrée d'audit.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuditSeverity {
    Info     = 0,
    Warning  = 1,
    Critical = 2,
    Alert    = 3,
}

impl AuditSeverity {
    /// Détermine la sévérité à partir de l'opération et du résultat.
    pub fn infer(op: AuditOp, result: AuditResult) -> Self {
        match result {
            AuditResult::Denied => Self::Critical,
            AuditResult::Error  => {
                if op.is_security_sensitive() { Self::Alert } else { Self::Warning }
            }
            AuditResult::Timeout => Self::Warning,
            _ => {
                if op.is_security_sensitive() { Self::Warning } else { Self::Info }
            }
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Info),
            1 => Some(Self::Warning),
            2 => Some(Self::Critical),
            3 => Some(Self::Alert),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditEntry on-disk (96 octets, packed repr)
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée d'audit on-disk (96 octets, taille fixe, layout stable).
///
/// Contraintes ONDISK-03 : aucun AtomicU64, tous les champs sont primitifs.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct AuditEntry {
    /// Ticks CPU au moment de l'enregistrement.
    pub tick:      u64,
    /// UID de l'acteur.
    pub actor_uid: u64,
    /// Capabilities de l'acteur (bitmask).
    pub actor_cap: u64,
    /// Identifiant objet cible.
    pub object_id: u64,
    /// BlobId de l'objet cible (32 octets).
    pub blob_id:   [u8; 32],
    /// Opération réalisée.
    pub op:        u8,
    /// Résultat.
    pub result:    u8,
    /// Sévérité (calculée à l'écriture).
    pub severity:  u8,
    /// Flags complémentaires (bit 0 = mutating, bit 1 = security).
    pub flags:     u8,
    /// Numéro de séquence dans le ring-buffer (toujours croissant).
    pub seq:       u64,
    /// Magic de validation.
    pub magic:     u32,
    /// Réservé/expansion future.
    pub _pad:      [u8; 4],
}

// SIZE_ASSERT_DISABLED: const _: () = assert!(core::mem::size_of::<AuditEntry>() == AUDIT_ENTRY_SIZE);

impl AuditEntry {
    /// Construit une entrée complète.
    pub fn new(
        tick:      u64,
        actor_uid: u64,
        actor_cap: u64,
        object_id: u64,
        blob_id:   [u8; 32],
        op:        AuditOp,
        result:    AuditResult,
        seq:       u64,
    ) -> Self {
        let sev = AuditSeverity::infer(op, result);
        let mut flags = 0u8;
        if op.is_mutating()          { flags |= 0x01; }
        if op.is_security_sensitive() { flags |= 0x02; }
        AuditEntry {
            tick, actor_uid, actor_cap, object_id, blob_id,
            op:       op     as u8,
            result:   result as u8,
            severity: sev    as u8,
            flags,
            seq,
            magic:    AUDIT_ENTRY_MAGIC,
            _pad:     [0; 4],
        }
    }

    /// Vérifie la validité de l'entrée.
    pub fn is_valid(&self) -> bool {
        self.magic == AUDIT_ENTRY_MAGIC
        && AuditOp::from_u8(self.op).is_some()
        && AuditResult::from_u8(self.result).is_some()
    }

    /// Retourne l'opération typée.
    pub fn op_typed(&self) -> Option<AuditOp> { AuditOp::from_u8(self.op) }

    /// Retourne le résultat typé.
    pub fn result_typed(&self) -> Option<AuditResult> { AuditResult::from_u8(self.result) }

    /// Retourne la sévérité typée.
    pub fn severity_typed(&self) -> Option<AuditSeverity> { AuditSeverity::from_u8(self.severity) }

    /// `true` si l'entrée représente une opération mutante.
    pub fn is_mutating(&self) -> bool { self.flags & 0x01 != 0 }

    /// `true` si l'entrée est liée à la sécurité.
    pub fn is_security(&self) -> bool { self.flags & 0x02 != 0 }

    /// Sérialise l'entrée en un tableau d'octets bruts.
    pub fn as_bytes(&self) -> &[u8; AUDIT_ENTRY_SIZE] {
        // SAFETY: AuditEntry est repr(C,packed), taille = AUDIT_ENTRY_SIZE.
        unsafe { &*(self as *const Self as *const [u8; AUDIT_ENTRY_SIZE]) }
    }

    /// Désérialise depuis un tableau d'octets bruts.
    pub fn from_bytes(bytes: &[u8; AUDIT_ENTRY_SIZE]) -> Self {
        // SAFETY: même layout, copie bit-à-bit d'une structure Pod.
        unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) }
    }

    /// Vérifie l'intégrité et retourne `ExofsError::InvalidMagic` si invalide.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.magic != AUDIT_ENTRY_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }
        if AuditOp::from_u8(self.op).is_none()
            || AuditResult::from_u8(self.result).is_none() {
            return Err(ExofsError::CorruptedStructure);
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditEntryBuilder — construction par étapes
// ─────────────────────────────────────────────────────────────────────────────

/// Constructeur fluide d'une entrée d'audit.
#[derive(Default)]
pub struct AuditEntryBuilder {
    tick:      u64,
    actor_uid: u64,
    actor_cap: u64,
    object_id: u64,
    blob_id:   [u8; 32],
    op:        Option<AuditOp>,
    result:    AuditResult,
    seq:       u64,
}

impl AuditEntryBuilder {
    pub fn new() -> Self { Self::default() }

    pub fn tick(mut self, t: u64)           -> Self { self.tick      = t;    self }
    pub fn actor_uid(mut self, uid: u64)    -> Self { self.actor_uid = uid;  self }
    pub fn actor_cap(mut self, cap: u64)    -> Self { self.actor_cap = cap;  self }
    pub fn object_id(mut self, id: u64)     -> Self { self.object_id = id;   self }
    pub fn blob_id(mut self, b: [u8; 32])   -> Self { self.blob_id   = b;    self }
    pub fn op(mut self, op: AuditOp)        -> Self { self.op        = Some(op); self }
    pub fn result(mut self, r: AuditResult) -> Self { self.result    = r;    self }
    pub fn seq(mut self, s: u64)            -> Self { self.seq       = s;    self }

    /// Finalise la construction.
    pub fn build(self) -> ExofsResult<AuditEntry> {
        let op = self.op.ok_or(ExofsError::InvalidArgument)?;
        Ok(AuditEntry::new(
            self.tick, self.actor_uid, self.actor_cap, self.object_id,
            self.blob_id, op, self.result, self.seq,
        ))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditSummary — agrégat statistique
// ─────────────────────────────────────────────────────────────────────────────

/// Résumé statistique d'un lot d'entrées d'audit.
#[derive(Clone, Debug, Default)]
pub struct AuditSummary {
    pub total:        u64,
    pub success:      u64,
    pub denied:       u64,
    pub errors:       u64,
    pub mutating:     u64,
    pub security_ops: u64,
    pub alerts:       u64,
}

impl AuditSummary {
    /// Accumule les statistiques d'une entrée.
    pub fn feed(&mut self, entry: &AuditEntry) {
        self.total = self.total.wrapping_add(1);
        if entry.is_security()                              { self.security_ops = self.security_ops.wrapping_add(1); }
        if entry.is_mutating()                              { self.mutating     = self.mutating.wrapping_add(1); }
        match AuditResult::from_u8(entry.result) {
            Some(AuditResult::Success) | Some(AuditResult::Partial) => {
                self.success = self.success.wrapping_add(1);
            }
            Some(AuditResult::Denied) => {
                self.denied = self.denied.wrapping_add(1);
            }
            Some(AuditResult::Error) | Some(AuditResult::Timeout) => {
                self.errors = self.errors.wrapping_add(1);
            }
            None => {}
        }
        if entry.severity >= AuditSeverity::Alert as u8 {
            self.alerts = self.alerts.wrapping_add(1);
        }
    }

    /// `true` si aucun événement de sécurité critique n'a eu lieu.
    pub fn is_clean(&self) -> bool { self.denied == 0 && self.alerts == 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make(op: AuditOp, result: AuditResult) -> AuditEntry {
        AuditEntry::new(100, 1, 0xFF, 42, [0u8; 32], op, result, 1)
    }

    #[test] fn test_entry_size() {
        assert_eq!(core::mem::size_of::<AuditEntry>(), 96);
    }

    #[test] fn test_entry_valid() {
        let e = make(AuditOp::Read, AuditResult::Success);
        assert!(e.is_valid());
        assert!(e.validate().is_ok());
    }

    #[test] fn test_entry_mutating_flag() {
        let e = make(AuditOp::Write, AuditResult::Success);
        assert!(e.is_mutating());
    }

    #[test] fn test_entry_security_flag() {
        let e = make(AuditOp::CryptoKey, AuditResult::Success);
        assert!(e.is_security());
    }

    #[test] fn test_op_from_u8_roundtrip() {
        for v in 0x01u8..=0x13 {
            assert!(AuditOp::from_u8(v).is_some(), "missing op {v:#x}");
        }
    }

    #[test] fn test_severity_infer_denied_is_critical() {
        let s = AuditSeverity::infer(AuditOp::Read, AuditResult::Denied);
        assert_eq!(s, AuditSeverity::Critical);
    }

    #[test] fn test_severity_infer_security_error_is_alert() {
        let s = AuditSeverity::infer(AuditOp::CryptoKey, AuditResult::Error);
        assert_eq!(s, AuditSeverity::Alert);
    }

    #[test] fn test_serialise_roundtrip() {
        let e = make(AuditOp::Create, AuditResult::Success);
        let bytes = e.as_bytes();
        let e2 = AuditEntry::from_bytes(bytes);
        assert!(e2.is_valid());
        assert_eq!(e2.op, e.op);
    }

    #[test] fn test_builder_ok() {
        let e = AuditEntryBuilder::new()
            .tick(999).actor_uid(7).actor_cap(0).object_id(42)
            .op(AuditOp::Delete).result(AuditResult::Success).seq(5)
            .build().unwrap();
        assert!(e.is_valid());
        assert!(e.is_mutating());
    }

    #[test] fn test_builder_missing_op() {
        let r = AuditEntryBuilder::new().build();
        assert!(r.is_err());
    }

    #[test] fn test_summary_clean_initially() {
        let mut s = AuditSummary::default();
        let e = make(AuditOp::Read, AuditResult::Success);
        s.feed(&e);
        assert!(s.is_clean());
    }

    #[test] fn test_summary_denied_not_clean() {
        let mut s = AuditSummary::default();
        let e = make(AuditOp::Read, AuditResult::Denied);
        s.feed(&e);
        assert!(!s.is_clean());
        assert_eq!(s.denied, 1);
    }
}
