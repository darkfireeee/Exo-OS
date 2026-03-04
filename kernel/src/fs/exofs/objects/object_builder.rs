// SPDX-License-Identifier: MIT
// ExoFS — object_builder.rs
// ObjectBuilder — construction fluent et validée d'un LogicalObject.
//
// Règles :
//   HASH-01  : BlobId = Blake3(raw_data) calculé ici, avant tout autre usage
//   REFCNT-01: compteur initialisé à 1, jamais à 0
//   OOM-02   : try_reserve partout
//   ARITH-02 : checked_add / saturating_* partout
//   DAG-01   : pas d'import storage/, ipc/, process/, arch/

#![allow(dead_code)]

use core::fmt;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use alloc::sync::Arc;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, ObjectId, BlobId, EpochId, DiskOffset,
    compute_blob_id, new_class1, new_class2,
    INLINE_DATA_MAX,
};
use crate::fs::exofs::core::object_kind::ObjectKind;
use crate::fs::exofs::core::object_class::ObjectClass;
use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::objects::logical_object::{LogicalObject, LogicalObjectRef};
use crate::fs::exofs::objects::object_meta::ObjectMeta;
use crate::fs::exofs::objects::inline_data::InlineData;
use crate::fs::exofs::objects::physical_ref::PhysicalRef;
use crate::fs::exofs::objects::extent_tree::ExtentTree;
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;
use crate::scheduler::sync::rwlock::RwLock;

// ── Erreurs de build ───────────────────────────────────────────────────────────

/// Erreurs spécifiques à la construction d'un LogicalObject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// Kind invalide pour la class demandée.
    KindClassMismatch,
    /// Données trop grandes pour un Blob.
    DataTooLarge,
    /// Mode POSIX invalide.
    InvalidMode,
    /// Epoch non initialisée (0).
    ZeroEpoch,
    /// Heap insuffisant.
    OutOfMemory,
    /// Paramètre obligatoire manquant.
    MissingRequired,
    /// PathIndex doit être Class2 (LOBJ-01).
    PathIndexMustBeClass2,
    /// Erreur ExoFS sous-jacente.
    Exofs(ExofsError),
}

impl From<ExofsError> for BuildError {
    fn from(e: ExofsError) -> Self {
        Self::Exofs(e)
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KindClassMismatch    => write!(f, "kind/class incompatibles"),
            Self::DataTooLarge         => write!(f, "données trop grandes"),
            Self::InvalidMode          => write!(f, "mode POSIX invalide"),
            Self::ZeroEpoch            => write!(f, "epoch = 0, invalide"),
            Self::OutOfMemory          => write!(f, "heap insuffisant"),
            Self::MissingRequired      => write!(f, "paramètre obligatoire absent"),
            Self::PathIndexMustBeClass2=> write!(f, "PathIndex doit être Class2 (LOBJ-01)"),
            Self::Exofs(e)             => write!(f, "ExofsError: {:?}", e),
        }
    }
}

pub type BuildResult<T> = Result<T, BuildError>;

// ── Paramètres de build ────────────────────────────────────────────────────────

/// Paramètres pour la création d'un nouveau LogicalObject.
pub struct BuildParams {
    /// Kind de l'objet.
    pub kind:       ObjectKind,
    /// Class de l'objet.
    pub class:      ObjectClass,
    /// Mode POSIX (0o644 pour fichiers, 0o755 pour répertoires).
    pub mode:       u32,
    /// UID du propriétaire.
    pub uid:        u32,
    /// GID du propriétaire.
    pub gid:        u32,
    /// Epoch de création.
    pub epoch:      EpochId,
    /// Bytes de la capability propriétaire (pour Class1).
    pub cap_bytes:  [u8; 32],
    /// Flags supplémentaires.
    pub extra_flags:ObjectFlags,
}

impl BuildParams {
    pub fn new(kind: ObjectKind, class: ObjectClass, epoch: EpochId) -> Self {
        Self {
            kind,
            class,
            mode:        if matches!(kind, ObjectKind::PathIndex) { 0o755 } else { 0o644 },
            uid:         0,
            gid:         0,
            epoch,
            cap_bytes:   [0u8; 32],
            extra_flags: ObjectFlags(0),
        }
    }

    /// Valide les paramètres.
    pub fn validate(&self) -> BuildResult<()> {
        if self.epoch.0 == 0 {
            return Err(BuildError::ZeroEpoch);
        }
        // LOBJ-01 : PathIndex DOIT être Class2.
        if matches!(self.kind, ObjectKind::PathIndex)
            && !matches!(self.class, ObjectClass::Class2)
        {
            return Err(BuildError::PathIndexMustBeClass2);
        }
        Ok(())
    }
}

// ── ObjectBuilder ──────────────────────────────────────────────────────────────

/// Constructeur fluent pour un LogicalObject.
///
/// Unique point d'entrée pour créer un objet ExoFS.
/// Règle SECURITY-01 : la vérification de capability est faite PAR L'APPELANT
/// avant d'appeler `build_with_data()` ou `build_empty()`.
pub struct ObjectBuilder {
    params: BuildParams,
}

impl ObjectBuilder {
    /// Crée un builder avec les paramètres.
    pub fn new(kind: ObjectKind, class: ObjectClass, epoch: EpochId) -> Self {
        Self { params: BuildParams::new(kind, class, epoch) }
    }

    /// Définit le mode POSIX.
    pub fn with_mode(mut self, mode: u32) -> Self {
        self.params.mode = mode;
        self
    }

    /// Définit uid/gid.
    pub fn with_owner(mut self, uid: u32, gid: u32) -> Self {
        self.params.uid = uid;
        self.params.gid = gid;
        self
    }

    /// Définit les bytes de la capability (pour Class1 ObjectId).
    pub fn with_cap_bytes(mut self, bytes: [u8; 32]) -> Self {
        self.params.cap_bytes = bytes;
        self
    }

    /// Ajoute des flags supplémentaires.
    pub fn with_flags(mut self, flags: ObjectFlags) -> Self {
        self.params.extra_flags = flags;
        self
    }

    /// Définit l'epoch.
    pub fn with_epoch(mut self, epoch: EpochId) -> Self {
        self.params.epoch = epoch;
        self
    }

    // ── Build avec données ─────────────────────────────────────────────────────

    /// Construit un LogicalObject avec les données fournies.
    ///
    /// HASH-01 : BlobId calculé ici sur `raw_data` brut AVANT tout autre usage.
    /// REFCNT-01 : ref_count initialisé à 1.
    pub fn build_with_data(
        self,
        raw_data: &[u8],
    ) -> BuildResult<(LogicalObjectRef, BlobId)> {
        self.params.validate()?;

        let epoch    = self.params.epoch;
        let data_len = raw_data.len() as u64;

        // HASH-01 : BlobId sur données brutes.
        let blob_id = compute_blob_id(raw_data);

        // Génération de l'ObjectId selon la class.
        let object_id = match self.params.class {
            ObjectClass::Class1 => new_class1(&blob_id, &self.params.cap_bytes),
            ObjectClass::Class2 => new_class2(),
        };

        // Détermine si les données tiennent en inline.
        let is_inline = data_len <= INLINE_DATA_MAX as u64;

        let mut flags = ObjectFlags(self.params.extra_flags.0);
        let physical_ref;

        if is_inline {
            flags = ObjectFlags(flags.0 | ObjectFlags::INLINE_DATA.0);
            let inline = InlineData::from_slice(raw_data).map_err(BuildError::Exofs)?;
            physical_ref = PhysicalRef::from_inline_data(inline.as_slice());
        } else {
            physical_ref = Ok(PhysicalRef::empty());
        }

        let now_tsc = epoch.0.wrapping_mul(1000);

        let meta = ObjectMeta {
            mode:           self.params.mode,
            uid:            self.params.uid,
            gid:            self.params.gid,
            nlink:          1,
            atime_tsc:      now_tsc,
            mtime_tsc:      now_tsc,
            ctime_tsc:      now_tsc,
            mime_type:      [0u8; 64],
            mime_len:       0,
            owner_cap_hash: self.params.cap_bytes,
            extra_flags:    0,
            xattrs:         core::array::from_fn(|_| {
                crate::fs::exofs::objects::object_meta::XAttrEntry::empty()
            }),
            xattr_count:    0,
        };

        let obj = LogicalObject {
            object_id,
            kind:         self.params.kind,
            class:        self.params.class,
            flags,
            ref_count:    AtomicU32::new(1),
            epoch_last:   AtomicU64::new(epoch.0),
            link_count:   AtomicU32::new(1),
            blob_id,
            epoch_create: epoch,
            disk_offset:  DiskOffset(0), // rempli après écriture physique
            data_size:    data_len,
            generation:   0,
            meta,
            physical_ref: physical_ref?,
            extent_tree:  ExtentTree::new(),
        };

        EPOCH_STATS.inc_objects_created();

        let arc = Arc::new(RwLock::new(obj));
        Ok((arc, blob_id))
    }

    /// Construit un objet vide (répertoire, PathIndex, …).
    ///
    /// REFCNT-01 : ref_count initialisé à 1.
    pub fn build_empty(self) -> BuildResult<LogicalObjectRef> {
        self.params.validate()?;

        let epoch = self.params.epoch;
        let object_id = match self.params.class {
            ObjectClass::Class1 => new_class1(&BlobId([0u8; 32]), &self.params.cap_bytes),
            ObjectClass::Class2 => new_class2(),
        };

        let now_tsc = epoch.0.wrapping_mul(1000);

        let meta = ObjectMeta {
            mode:           self.params.mode,
            uid:            self.params.uid,
            gid:            self.params.gid,
            nlink:          1,
            atime_tsc:      now_tsc,
            mtime_tsc:      now_tsc,
            ctime_tsc:      now_tsc,
            mime_type:      [0u8; 64],
            mime_len:       0,
            owner_cap_hash: self.params.cap_bytes,
            extra_flags:    0,
            xattrs:         core::array::from_fn(|_| {
                crate::fs::exofs::objects::object_meta::XAttrEntry::empty()
            }),
            xattr_count:    0,
        };

        let obj = LogicalObject {
            object_id,
            kind:         self.params.kind,
            class:        self.params.class,
            flags:        ObjectFlags(self.params.extra_flags.0),
            ref_count:    AtomicU32::new(1),
            epoch_last:   AtomicU64::new(epoch.0),
            link_count:   AtomicU32::new(1),
            blob_id:      BlobId([0u8; 32]),
            epoch_create: epoch,
            disk_offset:  DiskOffset(0),
            data_size:    0,
            generation:   0,
            meta,
            physical_ref: PhysicalRef::empty(),
            extent_tree:  ExtentTree::new(),
        };

        EPOCH_STATS.inc_objects_created();

        Ok(Arc::new(RwLock::new(obj)))
    }

    // ── Builders spécialisés (factory methods) ─────────────────────────────────

    /// Construit un objet Blob Class1 depuis des données brutes.
    pub fn blob_class1(
        data:      &[u8],
        cap_bytes: [u8; 32],
        uid:       u32,
        gid:       u32,
        epoch:     EpochId,
    ) -> BuildResult<(LogicalObjectRef, BlobId)> {
        ObjectBuilder::new(ObjectKind::Blob, ObjectClass::Class1, epoch)
            .with_owner(uid, gid)
            .with_cap_bytes(cap_bytes)
            .build_with_data(data)
    }

    /// Construit un objet PathIndex Class2 vide.
    ///
    /// LOBJ-01 : toujours Class2.
    pub fn path_index(uid: u32, gid: u32, epoch: EpochId) -> BuildResult<LogicalObjectRef> {
        ObjectBuilder::new(ObjectKind::PathIndex, ObjectClass::Class2, epoch)
            .with_owner(uid, gid)
            .with_mode(0o755)
            .build_empty()
    }

    /// Construit un objet Code Class1 depuis un ELF déjà validé.
    pub fn code_class1(
        elf_data:  &[u8],
        cap_bytes: [u8; 32],
        uid:       u32,
        gid:       u32,
        epoch:     EpochId,
    ) -> BuildResult<(LogicalObjectRef, BlobId)> {
        ObjectBuilder::new(ObjectKind::Code, ObjectClass::Class1, epoch)
            .with_owner(uid, gid)
            .with_cap_bytes(cap_bytes)
            .with_mode(0o755)
            .build_with_data(elf_data)
    }

    /// Construit un objet Config Class2 vide.
    pub fn config(uid: u32, gid: u32, epoch: EpochId) -> BuildResult<LogicalObjectRef> {
        ObjectBuilder::new(ObjectKind::Config, ObjectClass::Class2, epoch)
            .with_owner(uid, gid)
            .with_mode(0o644)
            .build_empty()
    }

    /// Construit un objet Secret Class2 (données passées chiffrées par l'appelant).
    pub fn secret(
        ciphertext: &[u8],
        uid:        u32,
        gid:        u32,
        epoch:      EpochId,
    ) -> BuildResult<(LogicalObjectRef, BlobId)> {
        ObjectBuilder::new(ObjectKind::Secret, ObjectClass::Class2, epoch)
            .with_owner(uid, gid)
            .with_mode(0o600)
            .with_flags(ObjectFlags::ENCRYPTED)
            .build_with_data(ciphertext)
    }
}

// ── BuildStats ─────────────────────────────────────────────────────────────────

/// Statistiques de construction d'objets.
#[derive(Default, Debug)]
pub struct BuildStats {
    pub total_built:    u64,
    pub inline_built:   u64,
    pub blob_built:     u64,
    pub empty_built:    u64,
    pub error_count:    u64,
    pub class1_built:   u64,
    pub class2_built:   u64,
}

impl BuildStats {
    pub fn new() -> Self { Self::default() }

    pub fn record_success(&mut self, is_inline: bool, is_empty: bool, class: &ObjectClass) {
        self.total_built = self.total_built.saturating_add(1);
        if is_empty         { self.empty_built  = self.empty_built.saturating_add(1); }
        else if is_inline   { self.inline_built = self.inline_built.saturating_add(1); }
        else                { self.blob_built   = self.blob_built.saturating_add(1); }
        match class {
            ObjectClass::Class1 => self.class1_built = self.class1_built.saturating_add(1),
            ObjectClass::Class2 => self.class2_built = self.class2_built.saturating_add(1),
        }
    }

    pub fn record_error(&mut self) {
        self.error_count = self.error_count.saturating_add(1);
    }
}

impl fmt::Display for BuildStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BuildStats {{ total: {}, inline: {}, blob: {}, empty: {}, \
             class1: {}, class2: {}, errors: {} }}",
            self.total_built, self.inline_built, self.blob_built,
            self.empty_built, self.class1_built, self.class2_built, self.error_count,
        )
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_inline() {
        let data  = b"hello exofs";
        let epoch = EpochId(5);
        let (obj_ref, blob_id) = ObjectBuilder::new(
            ObjectKind::Blob, ObjectClass::Class1, epoch,
        )
        .with_owner(1000, 100)
        .build_with_data(data)
        .unwrap();

        let obj = obj_ref.read();
        assert_eq!(obj.data_size, data.len() as u64);
        assert!(obj.is_inline());
        assert_eq!(obj.blob_id, compute_blob_id(data));
        drop(obj);
        drop(obj_ref);
        let _ = blob_id;
    }

    #[test]
    fn test_build_empty() {
        let epoch = EpochId(1);
        let obj_ref = ObjectBuilder::new(ObjectKind::PathIndex, ObjectClass::Class2, epoch)
            .build_empty()
            .unwrap();
        let obj = obj_ref.read();
        assert_eq!(obj.data_size, 0);
        assert!(!obj.is_inline());
        assert!(obj.is_class2());
    }

    #[test]
    fn test_lobj01_path_index_class2() {
        let epoch = EpochId(1);
        // PathIndex class1 doit échouer (LOBJ-01).
        let res = ObjectBuilder::new(ObjectKind::PathIndex, ObjectClass::Class1, epoch)
            .build_empty();
        assert!(res.is_err());
    }

    #[test]
    fn test_zero_epoch_rejected() {
        let epoch = EpochId(0);
        let res = ObjectBuilder::new(ObjectKind::Blob, ObjectClass::Class1, epoch)
            .build_empty();
        assert!(res.is_err());
    }

    #[test]
    fn test_blob_class1_factory() {
        let data = b"executable code here";
        let (obj_ref, _bid) = ObjectBuilder::blob_class1(
            data, [0u8; 32], 0, 0, EpochId(3),
        ).unwrap();
        let obj = obj_ref.read();
        assert!(matches!(obj.kind, ObjectKind::Blob));
        assert!(matches!(obj.class, ObjectClass::Class1));
    }
}
