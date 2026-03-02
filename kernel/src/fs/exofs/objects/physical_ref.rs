// kernel/src/fs/exofs/objects/physical_ref.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PhysicalRef — référence typée à une ressource physique ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un PhysicalRef est un discriminant qui permet à un LogicalObject de pointer
// vers soit un P-Blob partagé, soit des données inline, soit une valeur nulle.

use alloc::sync::Arc;

use crate::fs::exofs::core::{BlobId, DiskOffset};
use crate::fs::exofs::objects::physical_blob::PhysicalBlobInMemory;

/// Référence à la ressource physique d'un objet.
#[derive(Clone)]
pub enum PhysicalRef {
    /// Données stockées dans un P-Blob external (dédupliqué ou non).
    Blob(Arc<PhysicalBlobInMemory>),
    /// Données inline dans le LogicalObject (< INLINE_DATA_MAX).
    Inline,
    /// Objet sans données (métadonnées seules, ex. répertoire vide).
    Empty,
}

impl PhysicalRef {
    /// Retourne le BlobId si la référence pointe vers un P-Blob.
    pub fn blob_id(&self) -> Option<BlobId> {
        match self {
            PhysicalRef::Blob(blob) => Some(blob.blob_id),
            _ => None,
        }
    }

    /// Retourne l'offset disque du P-Blob si disponible.
    pub fn disk_offset(&self) -> Option<DiskOffset> {
        match self {
            PhysicalRef::Blob(blob) => Some(blob.data_offset),
            _ => None,
        }
    }

    /// Vrai si la référence pointe vers un P-Blob.
    #[inline]
    pub fn is_blob(&self) -> bool {
        matches!(self, PhysicalRef::Blob(_))
    }

    /// Vrai si les données sont inline.
    #[inline]
    pub fn is_inline(&self) -> bool {
        matches!(self, PhysicalRef::Inline)
    }

    /// Vrai si l'objet est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self, PhysicalRef::Empty)
    }
}
