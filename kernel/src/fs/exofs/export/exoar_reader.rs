//! exoar_reader.rs — Lecture d'archives ExoAR (no_std, RÈGLE 8).

use crate::fs::exofs::core::{BlobId, FsError};
use super::exoar_format::{
    ExoarHeader, ExoarEntryHeader, ExoarFooter,
    EXOAR_MAGIC, EXOAR_ENTRY_MAGIC, EXOAR_FOOTER_MAGIC,
};
use core::mem::size_of;

/// Trait de lecture séquentielle pour l'import.
pub trait ArchiveSource {
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), FsError>;
}

/// Récepteur de blobs lors de la lecture.
pub trait BlobReceiver {
    fn receive_blob(&mut self, id: BlobId, data: &[u8], raw_len: u64, flags: u8) -> Result<(), FsError>;
}

pub struct ExoarReader;

impl ExoarReader {
    /// Lit et valide l'en-tête de l'archive.  RÈGLE 8: magic en premier.
    pub fn read_header(src: &mut dyn ArchiveSource) -> Result<ExoarHeader, FsError> {
        let mut buf = [0u8; size_of::<ExoarHeader>()];
        src.read_exact(&mut buf)?;
        // SAFETY: buf a exactement la taille de ExoarHeader (repr(C), 96B).
        let hdr: ExoarHeader = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const _) };
        // RÈGLE 8 : magic EN PREMIER.
        if hdr.magic != EXOAR_MAGIC {
            return Err(FsError::InvalidMagic);
        }
        Ok(hdr)
    }

    /// Lit toutes les entrées jusqu'au footer et appelle `receiver` pour chaque blob.
    pub fn extract_all(
        src:      &mut dyn ArchiveSource,
        receiver: &mut dyn BlobReceiver,
    ) -> Result<u64, FsError> {
        let mut count: u64 = 0;
        loop {
            let mut eh_buf = [0u8; size_of::<ExoarEntryHeader>()];
            src.read_exact(&mut eh_buf)?;

            // Lit les 4 premiers octets pour discriminer entrée vs footer.
            let magic_u32 = u32::from_le_bytes([eh_buf[0], eh_buf[1], eh_buf[2], eh_buf[3]]);

            if magic_u32 == EXOAR_FOOTER_MAGIC {
                // Footer atteint, on s'arrête.
                break;
            }
            if magic_u32 != EXOAR_ENTRY_MAGIC {
                return Err(FsError::InvalidMagic);
            }

            // SAFETY: repr(C), 80B.
            let eh: ExoarEntryHeader = unsafe { core::ptr::read_unaligned(eh_buf.as_ptr() as *const _) };

            let payload_len = eh.payload_len as usize;
            let mut payload = alloc::vec![0u8; payload_len];
            src.read_exact(&mut payload)?;

            // Vérification CRC.
            let got_crc = crate::fs::exofs::core::crc32c::crc32c_update(0, &payload);
            if got_crc != eh.checksum {
                return Err(FsError::IntegrityCheckFailed);
            }

            let id = BlobId::from_raw(eh.blob_id);
            receiver.receive_blob(id, &payload, eh.raw_len, eh.flags)?;
            count = count.saturating_add(1);
        }
        Ok(count)
    }
}
