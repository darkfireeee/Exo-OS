//! stream_export.rs — Export streamé de blobs ExoFS (no_std).

use crate::fs::exofs::core::{BlobId, EpochId, FsError};
use super::exoar_format::{ExoarEntryHeader, EXOAR_ENTRY_MAGIC};
use core::mem::size_of;

/// Trait d'accès aux données blob lors de l'export stream.
pub trait BlobDataProvider {
    fn list_blobs_since(&self, since_epoch: EpochId) -> alloc::vec::Vec<BlobId>;
    fn read_blob_data(&self, id: BlobId) -> Result<alloc::vec::Vec<u8>, FsError>;
}

/// Trait de sortie stream lors de l'export.
pub trait StreamSink {
    fn write_chunk(&mut self, data: &[u8]) -> Result<(), FsError>;
}

pub struct StreamExporter {
    session_id: u32,
}

impl StreamExporter {
    pub fn new(session_id: u32) -> Self { Self { session_id } }

    /// Exporte tous les blobs depuis `since_epoch` vers `sink`.
    pub fn export(
        &self,
        provider: &dyn BlobDataProvider,
        sink:     &mut dyn StreamSink,
        since:    EpochId,
    ) -> Result<u64, FsError> {
        let ids = provider.list_blobs_since(since);
        let mut count = 0u64;
        for id in &ids {
            let data = match provider.read_blob_data(*id) {
                Ok(d)  => d,
                Err(_) => continue,   // Blob disparu entre liste et lecture.
            };
            let checksum = crate::fs::exofs::core::crc32c::crc32c_update(0, &data);
            let eh = ExoarEntryHeader {
                magic:       EXOAR_ENTRY_MAGIC,
                flags:       0,
                _pad:        [0; 3],
                blob_id:     id.as_bytes(),
                payload_len: data.len() as u64,
                raw_len:     data.len() as u64,
                checksum,
                _pad2:       [0; 4],
            };
            // SAFETY: repr(C) 80B.
            let hdr_bytes = unsafe {
                core::slice::from_raw_parts(&eh as *const _ as *const u8, size_of::<ExoarEntryHeader>())
            };
            sink.write_chunk(hdr_bytes)?;
            sink.write_chunk(&data)?;
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    pub fn session_id(&self) -> u32 { self.session_id }
}
