// kernel/src/fs/exofs/objects/extent.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Extent — plage de données disque contiguë
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un Extent représente une plage [offset, offset+len) de données sur disque.
// Il est utilisé dans l'extent tree pour cartographier les données d'un objet.
//
// RÈGLE ARITH-01 : checked_add sur tous les calculs d'offset.

use core::mem::size_of;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, DiskOffset, Extent, blake3_hash,
};

// ─────────────────────────────────────────────────────────────────────────────
// ObjectExtentDisk — entrée on-disk dans l'extent tree
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée d'un extent dans l'extent tree d'un objet (on-disk).
#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct ObjectExtentDisk {
    /// Offset logique dans l'objet (en octets).
    pub logical_offset: u64,
    /// Offset disque du bloc de données.
    pub disk_offset:    u64,
    /// Longueur en octets.
    pub len:            u64,
    /// Flags : bit 0 = sparse, bit 1 = compressé.
    pub flags:          u8,
    /// _pad.
    pub _pad:           [u8; 7],
}

const _: () = assert!(
    size_of::<ObjectExtentDisk>() == 32,
    "ObjectExtentDisk doit être 32 octets"
);

impl ObjectExtentDisk {
    pub const FLAG_SPARSE:     u8 = 1 << 0;
    pub const FLAG_COMPRESSED: u8 = 1 << 1;

    /// Vrai si l'extent est sparse (trou de fichier).
    #[inline]
    pub fn is_sparse(self) -> bool {
        self.flags & Self::FLAG_SPARSE != 0
    }

    /// Vrai si l'extent est compressé.
    #[inline]
    pub fn is_compressed(self) -> bool {
        self.flags & Self::FLAG_COMPRESSED != 0
    }

    /// Calcule l'offset de fin de l'extent.
    ///
    /// RÈGLE ARITH-01 : checked_add.
    pub fn end_offset(self) -> ExofsResult<u64> {
        let offset = { self.logical_offset };
        let len    = { self.len };
        offset.checked_add(len).ok_or(ExofsError::OffsetOverflow)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectExtent — version in-memory
// ─────────────────────────────────────────────────────────────────────────────

/// Extent d'un objet in-memory.
#[derive(Copy, Clone, Debug)]
pub struct ObjectExtent {
    /// Offset logique dans l'objet.
    pub logical_offset: u64,
    /// Blocs disque correspondants.
    pub physical:       Extent,
    /// Flags.
    pub flags:          u8,
}

impl ObjectExtent {
    /// Construit depuis la version on-disk.
    pub fn from_disk(d: ObjectExtentDisk) -> Self {
        Self {
            logical_offset: { d.logical_offset },
            physical: Extent {
                offset: DiskOffset({ d.disk_offset }),
                len:    { d.len },
            },
            flags: { d.flags },
        }
    }

    /// Vrai si l'extent est sparse.
    #[inline]
    pub fn is_sparse(&self) -> bool {
        self.flags & ObjectExtentDisk::FLAG_SPARSE != 0
    }
}
