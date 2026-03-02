//! metadata_export.rs — Sérialisation des métadonnées ExoFS (no_std, format texte ASCII).

use alloc::vec::Vec;
use core::fmt::Write as FmtWrite;
use alloc::string::String;
use crate::fs::exofs::core::{BlobId, EpochId, FsError};

/// Récepteur de sortie texte no_std.
pub trait TextSink {
    fn write_str(&mut self, s: &str) -> Result<(), FsError>;
}

/// Exporteur de métadonnées en format clé=valeur ASCII (sans JSON/XML pour no_std).
pub struct MetadataExporter;

impl MetadataExporter {
    pub fn export_blob_meta(
        sink: &mut dyn TextSink,
        id: BlobId,
        size: u64,
        epoch_id: EpochId,
        ref_count: u32,
    ) -> Result<(), FsError> {
        let mut buf = String::new();
        let bytes = id.as_bytes();
        let _ = write!(buf, "[blob]\n");
        let _ = write!(buf, "id=");
        for b in &bytes { let _ = write!(buf, "{:02x}", b); }
        let _ = write!(buf, "\nsize={}\nepoch={}\nref_count={}\n\n", size, epoch_id.0, ref_count);
        sink.write_str(&buf)
    }

    pub fn export_snapshot_meta(
        sink: &mut dyn TextSink,
        snap_id: u64,
        epoch_id: EpochId,
        n_blobs: u64,
        protected: bool,
    ) -> Result<(), FsError> {
        let mut buf = String::new();
        let _ = write!(buf, "[snapshot]\n");
        let _ = write!(buf, "id={}\nepoch={}\nn_blobs={}\nprotected={}\n\n",
            snap_id, epoch_id.0, n_blobs, if protected { "true" } else { "false" });
        sink.write_str(&buf)
    }

    pub fn export_header(
        sink:     &mut dyn TextSink,
        epoch_id: EpochId,
        total_blobs: u64,
    ) -> Result<(), FsError> {
        let mut buf = String::new();
        let _ = write!(buf, "[exofs_metadata_export]\nversion=1\nepoch={}\ntotal_blobs={}\n\n",
            epoch_id.0, total_blobs);
        sink.write_str(&buf)
    }
}
