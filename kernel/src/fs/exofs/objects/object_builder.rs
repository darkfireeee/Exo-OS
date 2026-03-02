// kernel/src/fs/exofs/objects/object_builder.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ObjectBuilder — construction fluent d'un LogicalObject
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// L'ObjectBuilder est l'unique point d'entrée pour créer un nouvel objet.
// Il accepte les paramètres obligatoires (kind, class, data) et produit
// un LogicalObject prêt à être ajouté au delta de l'epoch courant.
//
// RÈGLE SECURITY-01 : capability token vérifié par l'appelant AVANT build().
// RÈGLE HASH-01    : BlobId calculé sur raw_data ici (avant tout autre usage).

use core::sync::atomic::AtomicU32;
use alloc::sync::Arc;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, ObjectId, BlobId, EpochId,
    compute_blob_id, new_class1, new_class2,
};
use crate::fs::exofs::core::object_kind::ObjectKind;
use crate::fs::exofs::core::object_class::ObjectClass;
use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::objects::logical_object::LogicalObject;
use crate::fs::exofs::objects::object_meta::ObjectMeta;
use crate::fs::exofs::objects::inline_data::InlineData;
use crate::fs::exofs::core::stats::EXOFS_STATS;
use crate::scheduler::sync::rwlock::RwLock;

// ─────────────────────────────────────────────────────────────────────────────
// ObjectBuilder
// ─────────────────────────────────────────────────────────────────────────────

/// Constructeur fluent pour un LogicalObject.
pub struct ObjectBuilder {
    kind:      ObjectKind,
    class:     ObjectClass,
    raw_data:  Option<&'static [u8]>, // référence temp pour build
    mode:      u32,
    flags:     ObjectFlags,
    epoch_id:  EpochId,
    cap_bytes: [u8; 32],
}

impl ObjectBuilder {
    /// Commence la construction d'un objet.
    pub fn new(kind: ObjectKind, class: ObjectClass, epoch_id: EpochId) -> Self {
        Self {
            kind,
            class,
            raw_data:  None,
            mode:      ObjectMeta::MODE_FILE,
            flags:     ObjectFlags(0),
            epoch_id,
            cap_bytes: [0u8; 32],
        }
    }

    /// Définit les flags.
    pub fn with_flags(mut self, flags: ObjectFlags) -> Self {
        self.flags = flags;
        self
    }

    /// Définit le mode POSIX.
    pub fn with_mode(mut self, mode: u32) -> Self {
        self.mode = mode;
        self
    }

    /// Définit les bytes du CapToken propriétaire (pour dériver l'ObjectId Class1).
    pub fn with_cap_bytes(mut self, bytes: [u8; 32]) -> Self {
        self.cap_bytes = bytes;
        self
    }

    /// Construit le LogicalObject avec les données fournies.
    ///
    /// RÈGLE HASH-01 : BlobId = compute_blob_id(raw_data).
    pub fn build_with_data(
        self,
        raw_data: &[u8],
    ) -> ExofsResult<(Arc<RwLock<LogicalObject>>, BlobId)> {
        // RÈGLE HASH-01 : BlobId sur données RAW.
        let blob_id = compute_blob_id(raw_data);

        // Génération de l'ObjectId selon la class.
        let object_id = match self.class {
            ObjectClass::Class1 => new_class1(blob_id, &self.cap_bytes),
            ObjectClass::Class2 => new_class2(),
        };

        let data_size   = raw_data.len() as u64;
        let is_inline   = data_size <= crate::fs::exofs::core::INLINE_DATA_MAX as u64;
        let mut flags   = self.flags;

        let inline_data = if is_inline {
            flags.set(ObjectFlags::INLINE_DATA);
            Some(InlineData::from_slice(raw_data)?)
        } else {
            None
        };

        let obj = LogicalObject {
            object_id,
            blob_id,
            epoch_create: self.epoch_id,
            epoch_modify: self.epoch_id,
            disk_offset:  crate::fs::exofs::core::DiskOffset(0), // rempli après écriture
            data_size,
            flags,
            kind:         self.kind,
            class:        self.class,
            meta:         ObjectMeta::default_for_object(self.mode),
            inline_data,
            ref_count:    AtomicU32::new(1),
        };

        EXOFS_STATS.inc_objects_created();

        Ok((Arc::new(RwLock::new(obj)), blob_id))
    }

    /// Construit un objet vide (ex. répertoire).
    pub fn build_empty(self) -> ExofsResult<Arc<RwLock<LogicalObject>>> {
        let object_id = match self.class {
            ObjectClass::Class1 => new_class1(BlobId([0u8; 32]), &self.cap_bytes),
            ObjectClass::Class2 => new_class2(),
        };
        let obj = LogicalObject {
            object_id,
            blob_id:      BlobId([0u8; 32]),
            epoch_create: self.epoch_id,
            epoch_modify: self.epoch_id,
            disk_offset:  crate::fs::exofs::core::DiskOffset(0),
            data_size:    0,
            flags:        self.flags,
            kind:         self.kind,
            class:        self.class,
            meta:         ObjectMeta::default_for_object(self.mode),
            inline_data:  None,
            ref_count:    AtomicU32::new(1),
        };
        EXOFS_STATS.inc_objects_created();
        Ok(Arc::new(RwLock::new(obj)))
    }
}
