// kernel/src/fs/exofs/objects/logical_object.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// LogicalObject — objet logique ExoFS (in-memory)
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un LogicalObject est la représentation RAM d'un objet ExoFS.
// Il correspond exactement à un ObjectHeader + données sur disque.
//
// RÈGLE ONDISK-01 : AtomicU32/u64 INTERDIT dans les structs on-disk.
// RÈGLE SECURITY-01 : capability token vérifié avant chaque accès.

use core::sync::atomic::{AtomicU32, Ordering};
use alloc::sync::Arc;

use crate::fs::exofs::core::{
    ObjectId, BlobId, EpochId, DiskOffset, ExofsError, ExofsResult,
};
use crate::fs::exofs::core::object_kind::ObjectKind;
use crate::fs::exofs::core::object_class::ObjectClass;
use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::objects::object_meta::ObjectMeta;
use crate::fs::exofs::objects::inline_data::InlineData;
use crate::scheduler::sync::rwlock::RwLock;

// ─────────────────────────────────────────────────────────────────────────────
// LogicalObjectDisk — version on-disk (types plain) — 256 octets
// ─────────────────────────────────────────────────────────────────────────────

/// Structure on-disk d'un LogicalObject.
///
/// RÈGLE ONDISK-01 : types plain uniquement — pas d'AtomicU64/Vec.
/// Stocké immédiatement après l'ObjectHeader sur disque.
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct LogicalObjectDisk {
    /// Identifiant de l'objet.
    pub object_id:    [u8; 32],
    /// BlobId du contenu (avant compression — RÈGLE HASH-01).
    pub blob_id:      [u8; 32],
    /// Epoch de création.
    pub epoch_create: u64,
    /// Epoch de dernière modification.
    pub epoch_modify: u64,
    /// Offset disque du P-Blob (0 si inline).
    pub blob_offset:  u64,
    /// Taille des données en octets.
    pub data_size:    u64,
    /// Flags (ObjectFlags.0 as u16).
    pub flags:        u16,
    /// Kind (ObjectKind #[repr(u8)]).
    pub kind:         u8,
    /// Class (ObjectClass #[repr(u8)]).
    pub class:        u8,
    /// Compteur de références on-disk (mis à jour lors d'un commit).
    pub ref_count:    u32,
    /// Droits POSIX simulés (mode bits 0-11).
    pub mode:         u32,
    /// UID/GID POSIX (capability-based dans ExoFS).
    pub uid:          u32,
    pub gid:          u32,
    /// _pad pour 128 octets.
    pub _pad:         [u8; 44],
    /// Checksum de ce sous-header.
    pub checksum:     [u8; 32],
}

const _: () = assert!(
    core::mem::size_of::<LogicalObjectDisk>() == 256,
    "LogicalObjectDisk doit être exactement 256 octets"
);

// ─────────────────────────────────────────────────────────────────────────────
// LogicalObject — version RAM
// ─────────────────────────────────────────────────────────────────────────────

/// LogicalObject in-memory.
///
/// Protégé par un RwLock pour les accès concurrents.
/// Le ref_count atomique est séparé du RwLock pour éviter les acquis à chaud.
#[repr(align(64))]
pub struct LogicalObject {
    /// Identifiant unique.
    pub object_id:    ObjectId,
    /// BlobId du contenu actuel.
    pub blob_id:      BlobId,
    /// Epoch de création.
    pub epoch_create: EpochId,
    /// Epoch de dernière modification.
    pub epoch_modify: EpochId,
    /// Offset disque de la version courante.
    pub disk_offset:  DiskOffset,
    /// Taille des données en octets.
    pub data_size:    u64,
    /// Flags de l'objet.
    pub flags:        ObjectFlags,
    /// Kind de l'objet.
    pub kind:         ObjectKind,
    /// Class de l'objet.
    pub class:        ObjectClass,
    /// Métadonnées étendues.
    pub meta:         ObjectMeta,
    /// Données inline (si INLINE_DATA flag actif).
    pub inline_data:  Option<InlineData>,
    /// Compteur de références (AtomicU32 pour les reads lock-free).
    pub ref_count:    AtomicU32,
}

impl LogicalObject {
    /// Crée un LogicalObject depuis son état on-disk.
    pub fn from_disk(disk: &LogicalObjectDisk) -> ExofsResult<Self> {
        let kind = ObjectKind::from_u8({ disk.kind })
            .ok_or(ExofsError::InvalidObjectKind)?;
        let class = match { disk.class } {
            1 => ObjectClass::Class1,
            2 => ObjectClass::Class2,
            _ => return Err(ExofsError::InvalidObjectClass),
        };
        Ok(Self {
            object_id:    ObjectId({ disk.object_id }),
            blob_id:      BlobId({ disk.blob_id }),
            epoch_create: EpochId({ disk.epoch_create }),
            epoch_modify: EpochId({ disk.epoch_modify }),
            disk_offset:  DiskOffset({ disk.blob_offset }),
            data_size:    { disk.data_size },
            flags:        ObjectFlags({ disk.flags }),
            kind,
            class,
            meta:         ObjectMeta::from_disk(disk),
            inline_data:  None,
            ref_count:    AtomicU32::new({ disk.ref_count }),
        })
    }

    /// Vrai si l'objet est supprimé logiquement.
    #[inline]
    pub fn is_deleted(&self) -> bool {
        self.flags.contains(ObjectFlags::DELETED)
    }

    /// Vrai si l'objet utilise des données inline.
    #[inline]
    pub fn is_inline(&self) -> bool {
        self.flags.contains(ObjectFlags::INLINE_DATA)
    }

    /// Incrémente le compteur de références.
    #[inline]
    pub fn inc_ref(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente le compteur de références.
    ///
    /// RÈGLE REFCNT-01 : panic si underflow (0 → decrement).
    #[inline]
    pub fn dec_ref(&self) -> u32 {
        let prev = self.ref_count.fetch_sub(1, Ordering::Release);
        if prev == 0 {
            panic!("ExoFS: dec_ref underflow sur ObjectId {:?}", self.object_id.0);
        }
        prev - 1
    }
}

/// Référence partagée à un LogicalObject — le type manipulé dans le kernel.
pub type LogicalObjectRef = Arc<RwLock<LogicalObject>>;
