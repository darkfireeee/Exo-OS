//! incremental_export.rs — Export incrémental depuis un epoch de référence (no_std).

use crate::fs::exofs::core::{BlobId, EpochId, FsError};
use super::export_audit::{EXPORT_AUDIT, ExportEvent};
use super::exoar_writer::{ExoarWriter, ArchiveSink};

/// Source de diff incrémental.
pub trait IncrementalBlobSource {
    /// Retourne les blobs créés ou modifiés depuis `since_epoch`.
    fn new_blobs_since(&self, since_epoch: EpochId) -> alloc::vec::Vec<(BlobId, u64 /* epoch_created */)>;
    fn read_blob_data(&self, id: BlobId) -> Result<alloc::vec::Vec<u8>, FsError>;
    fn current_epoch(&self) -> EpochId;
}

pub struct IncrementalExport {
    session_id: u32,
}

impl IncrementalExport {
    pub fn new(session_id: u32) -> Self { Self { session_id } }

    /// Exporte tous les blobs nouveaux depuis `base_epoch` vers `sink`.
    pub fn run(
        &self,
        source:     &dyn IncrementalBlobSource,
        sink:       &mut dyn ArchiveSink,
        base_epoch: EpochId,
    ) -> Result<u64, FsError> {
        let current = source.current_epoch();
        let mut writer = ExoarWriter::new();
        writer.write_header(sink, current, [0u8; 32])?;

        let candidates = source.new_blobs_since(base_epoch);
        let mut exported = 0u64;
        let mut total_bytes = 0u64;

        for (id, _epoch_created) in &candidates {
            let data = match source.read_blob_data(*id) {
                Ok(d)  => d,
                Err(_) => continue,
            };
            total_bytes = total_bytes.saturating_add(data.len() as u64);
            writer.add_blob(sink, *id, &data, data.len() as u64, 0)?;
            exported = exported.saturating_add(1);
        }
        writer.finalize(sink)?;
        EXPORT_AUDIT.push(ExportEvent::ExportCompleted, [0u8;32], exported, total_bytes, self.session_id, 0);
        Ok(exported)
    }
}
