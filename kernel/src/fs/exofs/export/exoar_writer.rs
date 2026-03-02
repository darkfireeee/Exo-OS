//! exoar_writer.rs — Création d'archives ExoAR (no_std).

use alloc::vec::Vec;
use crate::fs::exofs::core::{BlobId, EpochId, FsError};
use crate::arch::time::read_ticks;
use super::exoar_format::{
    ExoarHeader, ExoarEntryHeader, ExoarFooter,
    EXOAR_MAGIC, EXOAR_ENTRY_MAGIC, EXOAR_FOOTER_MAGIC, EXOAR_VERSION,
};

/// Trait d'écriture séquentielle pour l'export.
pub trait ArchiveSink {
    fn write_all(&mut self, data: &[u8]) -> Result<(), FsError>;
}

/// Écrivain d'archives ExoAR.
pub struct ExoarWriter {
    entry_count: u64,
    arch_crc:    u32,
}

impl ExoarWriter {
    pub fn new() -> Self {
        Self { entry_count: 0, arch_crc: 0 }
    }

    /// Écrit l'en-tête global.  Doit être appelé en premier.
    pub fn write_header(
        &mut self,
        sink: &mut dyn ArchiveSink,
        epoch_id: EpochId,
        archive_id: [u8; 32],
    ) -> Result<(), FsError> {
        let hdr = ExoarHeader {
            magic:       EXOAR_MAGIC,
            version:     EXOAR_VERSION,
            _pad:        [0; 6],
            entry_count: 0,   // mis à jour en footer
            epoch_id:    epoch_id.0,
            created_at:  read_ticks(),
            archive_id,
            flags:       0,
            _pad2:       [0; 20],
        };
        // SAFETY: ExoarHeader repr(C), taille connue.
        let bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const _ as *const u8, core::mem::size_of::<ExoarHeader>())
        };
        self.update_crc(bytes);
        sink.write_all(bytes)
    }

    /// Ajoute un blob dans l'archive.
    pub fn add_blob(
        &mut self,
        sink:    &mut dyn ArchiveSink,
        id:      BlobId,
        payload: &[u8],
        raw_len: u64,
        flags:   u8,
    ) -> Result<(), FsError> {
        let checksum = crate::fs::exofs::core::crc32c::crc32c_update(0, payload);
        let eh = ExoarEntryHeader {
            magic:       EXOAR_ENTRY_MAGIC,
            flags,
            _pad:        [0; 3],
            blob_id:     id.as_bytes(),
            payload_len: payload.len() as u64,
            raw_len,
            checksum,
            _pad2:       [0; 4],
        };
        // SAFETY: repr(C)
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&eh as *const _ as *const u8, core::mem::size_of::<ExoarEntryHeader>())
        };
        self.update_crc(hdr_bytes);
        self.update_crc(payload);
        sink.write_all(hdr_bytes)?;
        sink.write_all(payload)?;
        self.entry_count = self.entry_count.wrapping_add(1);
        Ok(())
    }

    /// Écrit le footer et finalise l'archive.
    pub fn finalize(&mut self, sink: &mut dyn ArchiveSink) -> Result<(), FsError> {
        let footer = ExoarFooter {
            magic: EXOAR_FOOTER_MAGIC,
            _pad:  [0; 4],
            crc32: self.arch_crc,
            _pad2: [0; 4],
        };
        // SAFETY: repr(C)
        let bytes = unsafe {
            core::slice::from_raw_parts(&footer as *const _ as *const u8, core::mem::size_of::<ExoarFooter>())
        };
        sink.write_all(bytes)
    }

    fn update_crc(&mut self, data: &[u8]) {
        self.arch_crc = crate::fs::exofs::core::crc32c::crc32c_update(self.arch_crc, data);
    }

    pub fn entry_count(&self) -> u64 { self.entry_count }
}
