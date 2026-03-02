// kernel/src/fs/exofs/storage/object_reader.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Lecture d'un objet logique depuis disque
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE HDR-03 : ObjectHeader.verify() AVANT tout accès aux données.
// RÈGLE OOM-02 : try_reserve avant allocation de Vec.
// RÈGLE SECURITY-01 : le vérificateur de BlobId est appelé après lecture.

use core::mem::size_of;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, ObjectId, BlobId, DiskOffset,
    verify_blob_id,
};
use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::storage::object_writer::ObjectHeader;
use crate::fs::exofs::core::stats::EXOFS_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// ObjectReadResult — résultat de la lecture
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une lecture d'objet réussie.
pub struct ObjectReadResult {
    /// ObjectId lu depuis l'en-tête.
    pub object_id:    ObjectId,
    /// BlobId lu depuis l'en-tête (non re-calculé — appeler verify_blob si besoin).
    pub blob_id:      BlobId,
    /// Flags de l'objet.
    pub flags:        ObjectFlags,
    /// Données brutes du payload.
    pub data:         Vec<u8>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Lecture d'un objet complet
// ─────────────────────────────────────────────────────────────────────────────

/// Lit et valide un objet entier depuis `disk_offset`.
///
/// # Protocole
/// 1. Lire 128 octets → ObjectHeader.
/// 2. verify() sur ObjectHeader (magic + checksum) — RÈGLE HDR-03.
/// 3. Lire payload_size octets supplémentaires.
/// 4. Vérifier BlobId si `verify_content` est vrai.
///
/// RÈGLE OOM-02 : try_reserve avant allocation.
pub fn read_object(
    disk_offset:    DiskOffset,
    verify_content: bool,
    read_fn:        &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
) -> ExofsResult<ObjectReadResult> {
    let header_size = size_of::<ObjectHeader>();

    // ── Étape 1 : lire l'en-tête ───────────────────────────────────────────
    let mut hdr_buf = [0u8; 128];
    let n = read_fn(disk_offset, &mut hdr_buf)?;
    if n != 128 {
        return Err(ExofsError::PartialWrite); // PartialRead == PartialWrite sémantiquement
    }

    // ── Étape 2 : parser et vérifier l'en-tête ────────────────────────────
    // RÈGLE HDR-03 : verify() AVANT tout accès aux champs.
    // SAFETY: hdr_buf est 128 octets, aligné pour ObjectHeader #[repr(C, packed)].
    let header: ObjectHeader = unsafe {
        core::ptr::read_unaligned(hdr_buf.as_ptr() as *const ObjectHeader)
    };
    header.verify()?;

    let payload_size = { header.payload_size } as usize;
    let object_id    = ObjectId({ header.object_id });
    let blob_id      = BlobId({ header.blob_id });
    let flags        = ObjectFlags({ header.flags });

    // ── Étape 3 : lire le payload ─────────────────────────────────────────
    let mut data: Vec<u8> = Vec::new();
    if payload_size > 0 {
        data.try_reserve(payload_size).map_err(|_| ExofsError::NoMemory)?;
        data.resize(payload_size, 0u8);

        let payload_offset = DiskOffset(
            disk_offset.0
                .checked_add(128)
                .ok_or(ExofsError::OffsetOverflow)?
        );
        let n = read_fn(payload_offset, &mut data)?;
        if n != payload_size {
            return Err(ExofsError::PartialWrite);
        }
    }

    // ── Étape 4 : vérifier le BlobId si demandé ───────────────────────────
    if verify_content && !data.is_empty() {
        if !verify_blob_id(&data, &blob_id) {
            return Err(ExofsError::BlobIdMismatch);
        }
    }

    // Statistiques.
    EXOFS_STATS.inc_objects_read();
    EXOFS_STATS.add_io_read(128 + payload_size as u64);

    Ok(ObjectReadResult { object_id, blob_id, flags, data })
}

/// Lit uniquement l'en-tête d'un objet (sans le payload).
///
/// Utile pour le GC ou le path resolver qui ont besoin des métadonnées
/// sans charger les données.
pub fn read_object_header(
    disk_offset: DiskOffset,
    read_fn:     &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
) -> ExofsResult<ObjectHeader> {
    let mut buf = [0u8; 128];
    let n = read_fn(disk_offset, &mut buf)?;
    if n != 128 {
        return Err(ExofsError::PartialWrite);
    }
    // SAFETY: buf est 128 octets, ObjectHeader #[repr(C, packed)] 128 octets.
    let header: ObjectHeader = unsafe {
        core::ptr::read_unaligned(buf.as_ptr() as *const ObjectHeader)
    };
    // RÈGLE HDR-03 : vérification obligatoire avant retour.
    header.verify()?;
    Ok(header)
}
