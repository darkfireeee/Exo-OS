//! stream_import.rs — Import streamé de blobs ExoFS (no_std, RÈGLES 8+11).

use crate::fs::exofs::core::{BlobId, FsError};
use super::exoar_format::{ExoarEntryHeader, EXOAR_ENTRY_MAGIC};
use super::export_audit::{EXPORT_AUDIT, ExportEvent};
use core::mem::size_of;

/// Trait d'écriture des blobs importés dans le FS.
pub trait BlobWriter {
    fn write_blob(&self, id: BlobId, data: &[u8]) -> Result<(), FsError>;
}

pub struct StreamImporter {
    session_id: u32,
}

impl StreamImporter {
    pub fn new(session_id: u32) -> Self { Self { session_id } }

    /// Importe un entry header + payload depuis `src`, écrit le blob via `writer`.
    /// RÈGLE 8: magic vérifié EN PREMIER.
    /// RÈGLE 11: BlobId recomputé = Blake3(données brutes) et comparé.
    pub fn import_entry(
        &self,
        src:    &mut dyn super::exoar_reader::ArchiveSource,
        writer: &dyn BlobWriter,
    ) -> Result<BlobId, FsError> {
        let mut eh_buf = [0u8; size_of::<ExoarEntryHeader>()];
        src.read_exact(&mut eh_buf)?;

        // RÈGLE 8: magic EN PREMIER.
        let got_magic = u32::from_le_bytes([eh_buf[0], eh_buf[1], eh_buf[2], eh_buf[3]]);
        if got_magic != EXOAR_ENTRY_MAGIC {
            return Err(FsError::InvalidMagic);
        }

        // SAFETY: repr(C) 80B, validé par magic.
        let eh: ExoarEntryHeader = unsafe { core::ptr::read_unaligned(eh_buf.as_ptr() as *const _) };

        let payload_len = eh.payload_len as usize;
        let mut payload = alloc::vec![0u8; payload_len];
        src.read_exact(&mut payload)?;

        // Vérification CRC.
        let got_crc = crate::fs::exofs::core::crc32c::crc32c_update(0, &payload);
        if got_crc != eh.checksum {
            EXPORT_AUDIT.push(ExportEvent::VerificationFailed, eh.blob_id, 1, payload_len as u64, self.session_id, -5);
            return Err(FsError::IntegrityCheckFailed);
        }

        // RÈGLE 11: BlobId = Blake3(données brutes, avant tout traitement).
        let expected_id = BlobId::from_bytes_blake3(&payload);
        let given_id    = BlobId::from_raw(eh.blob_id);
        if expected_id.as_bytes() != given_id.as_bytes() {
            EXPORT_AUDIT.push(ExportEvent::VerificationFailed, eh.blob_id, 1, payload_len as u64, self.session_id, -6);
            return Err(FsError::IntegrityCheckFailed);
        }

        writer.write_blob(given_id, &payload)?;
        EXPORT_AUDIT.push(ExportEvent::BlobImported, eh.blob_id, 1, payload_len as u64, self.session_id, 0);
        Ok(given_id)
    }

    pub fn session_id(&self) -> u32 { self.session_id }
}
