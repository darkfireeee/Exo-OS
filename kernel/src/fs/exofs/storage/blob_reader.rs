// kernel/src/fs/exofs/storage/blob_reader.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Lecture d'un P-Blob depuis disque
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE HDR-03 : BlobHeader.verify() AVANT tout accès aux données.
// RÈGLE OOM-02 : try_reserve avant allocation.

use core::mem::size_of;
use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, BlobId, DiskOffset, verify_blob_id,
};
use crate::fs::exofs::storage::blob_writer::BlobHeader;
use crate::fs::exofs::core::stats::EXOFS_STATS;

/// Résultat d'une lecture de P-Blob.
pub struct BlobReadResult {
    pub blob_id:      BlobId,
    pub ref_count:    u32,
    pub data:         Vec<u8>,
}

/// Lit un P-Blob depuis `disk_offset`.
///
/// RÈGLE HDR-03 : vérifie le BlobHeader avant de lire le payload.
pub fn read_blob(
    disk_offset:    DiskOffset,
    verify_content: bool,
    read_fn:        &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
) -> ExofsResult<BlobReadResult> {
    let header_size = size_of::<BlobHeader>();

    // Lecture de l'en-tête.
    let mut hdr_buf = [0u8; 96];
    let n = read_fn(disk_offset, &mut hdr_buf)?;
    if n != 96 {
        return Err(ExofsError::PartialWrite);
    }

    // RÈGLE HDR-03 : vérification AVANT accès aux champs.
    // SAFETY: BlobHeader est #[repr(C, packed)], taille 96.
    let header: BlobHeader = unsafe {
        core::ptr::read_unaligned(hdr_buf.as_ptr() as *const BlobHeader)
    };
    header.verify()?;

    let payload_size = { header.payload_size } as usize;
    let blob_id      = BlobId({ header.blob_id });
    let ref_count    = { header.ref_count };

    // Lecture du payload.
    let mut data: Vec<u8> = Vec::new();
    if payload_size > 0 {
        data.try_reserve(payload_size).map_err(|_| ExofsError::NoMemory)?;
        data.resize(payload_size, 0u8);
        let payload_offset = DiskOffset(
            disk_offset.0
                .checked_add(header_size as u64)
                .ok_or(ExofsError::OffsetOverflow)?
        );
        let n = read_fn(payload_offset, &mut data)?;
        if n != payload_size {
            return Err(ExofsError::PartialWrite);
        }
    }

    // Vérification optionnelle du BlobId.
    if verify_content && !data.is_empty() {
        if !verify_blob_id(&data, &blob_id) {
            return Err(ExofsError::BlobIdMismatch);
        }
    }

    EXOFS_STATS.add_io_read(header_size as u64 + payload_size as u64);

    Ok(BlobReadResult { blob_id, ref_count, data })
}
