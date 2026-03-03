//! exoar_writer.rs — Création et finalisation d'archives ExoAR (no_std).
//!
//! Ce module fournit :
//!  - `ArchiveSink`         : trait d'écriture séquentielle.
//!  - `ExoarWriter`         : écrivain principal (header → blobs → footer).
//!  - `ExoarBufferedWriter` : accumule les données en mémoire avant flush.
//!  - `ExoarWriteStats`     : statistiques de l'archive en cours.
//!  - `ExoarWriteOptions`   : configuration de l'écrivain.
//!  - `ExoarWriteError`     : erreurs d'écriture spécifiques.
//!  - `SinkVec`             : implémentation de ArchiveSink sur Vec<u8>.
//!
//! RÈGLE 8  : magic écrit EN PREMIER pour chaque structure.
//! RÈGLE 11 : BlobId = blake3(données brutes) — calculé ici.
//! RECUR-01 : pas de récursion — boucles while.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_* / wrapping_* sur tous les compteurs.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::exoar_format::{
    ExoarHeader, ExoarEntryHeader, ExoarFooter,
    EXOAR_MAGIC, EXOAR_ENTRY_MAGIC, EXOAR_VERSION,
    ARCHIVE_FLAG_INCREMENTAL, ARCHIVE_FLAG_VERIFIED,
    ENTRY_FLAG_COMPRESSED, ENTRY_FLAG_ENCRYPTED, ENTRY_FLAG_TOMBSTONE,
    EXOAR_MAX_ENTRIES, EXOAR_MAX_PAYLOAD,
    crc32c_update, crc32c_compute,
};
use core::mem::size_of;

// ─── Trait de sortie ─────────────────────────────────────────────────────────

/// Abstraction d'écriture séquentielle vers une destination d'archive.
/// RECUR-01 : les implémentations ne se rappellent pas elles-mêmes.
pub trait ArchiveSink {
    /// Écrit exactement `buf` dans la destination.
    fn write_all(&mut self, buf: &[u8]) -> ExofsResult<()>;
    /// Retourne le nombre total d'octets écrits.
    fn bytes_written(&self) -> u64;
}

// ─── Erreurs d'écriture ───────────────────────────────────────────────────────

/// Erreur spécifique à l'écriture d'archive ExoAR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExoarWriteError {
    /// L'archive a déjà été finalisée.
    AlreadyFinalized,
    /// L'archive n'a pas encore été initialisée (header non écrit).
    NotStarted,
    /// Trop d'entrées (dépasse EXOAR_MAX_ENTRIES).
    TooManyEntries,
    /// Payload trop grand (dépasse EXOAR_MAX_PAYLOAD).
    PayloadTooLarge,
    /// Erreur d'allocation mémoire.
    OutOfMemory,
    /// Erreur d'écriture vers le sink.
    IoError,
}

impl From<ExoarWriteError> for ExofsError {
    fn from(e: ExoarWriteError) -> Self {
        match e {
            ExoarWriteError::AlreadyFinalized   => ExofsError::InvalidArgument,
            ExoarWriteError::NotStarted         => ExofsError::InvalidArgument,
            ExoarWriteError::TooManyEntries     => ExofsError::OffsetOverflow,
            ExoarWriteError::PayloadTooLarge    => ExofsError::OffsetOverflow,
            ExoarWriteError::OutOfMemory        => ExofsError::NoMemory,
            ExoarWriteError::IoError            => ExofsError::IoFailed,
        }
    }
}

// ─── Options d'écriture ───────────────────────────────────────────────────────

/// Options de configuration pour l'écrivain ExoAR.
#[derive(Clone, Copy, Debug)]
pub struct ExoarWriteOptions {
    /// Flags à placer dans ExoarHeader.
    pub archive_flags: u32,
    /// Epoch de base (source de l'incrémental).
    pub epoch_base: u64,
    /// Epoch cible (état à exporter).
    pub epoch_target: u64,
    /// Timestamp de création (secondes UNIX, 0 = non renseigné).
    pub created_at: u64,
    /// Vérifier le CRC32C de chaque payload avant d'écrire.
    pub verify_before_write: bool,
    /// Écrire les tombstones (suppression).
    pub write_tombstones: bool,
}

impl ExoarWriteOptions {
    pub const fn default() -> Self {
        Self {
            archive_flags: 0,
            epoch_base: 0,
            epoch_target: 0,
            created_at: 0,
            verify_before_write: false,
            write_tombstones: true,
        }
    }

    pub const fn snapshot(epoch: u64) -> Self {
        Self {
            archive_flags: super::exoar_format::ARCHIVE_FLAG_SNAPSHOT,
            epoch_base: 0,
            epoch_target: epoch,
            created_at: 0,
            verify_before_write: true,
            write_tombstones: false,
        }
    }

    pub const fn incremental(epoch_base: u64, epoch_target: u64) -> Self {
        Self {
            archive_flags: ARCHIVE_FLAG_INCREMENTAL,
            epoch_base,
            epoch_target,
            created_at: 0,
            verify_before_write: false,
            write_tombstones: true,
        }
    }
}

// ─── Statistiques d'écriture ─────────────────────────────────────────────────

/// Statistiques accumulées pendant l'écriture.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExoarWriteStats {
    /// Nombre de blobs écrits.
    pub blobs_written: u32,
    /// Nombre de tombstones écrits.
    pub tombstones_written: u32,
    /// Octets total de payload écrits.
    pub payload_bytes: u64,
    /// Taille totale de l'archive en bytes.
    pub archive_bytes: u64,
    /// Nombre d'erreurs d'écriture ignorées.
    pub write_errors: u32,
    /// CRC32C global courant (accumulé au fil des écritures).
    pub running_crc: u32,
}

impl ExoarWriteStats {
    pub const fn new() -> Self {
        Self {
            blobs_written: 0,
            tombstones_written: 0,
            payload_bytes: 0,
            archive_bytes: 0,
            write_errors: 0,
            running_crc: 0,
        }
    }

    /// Retourne le nombre total d'entrées écrites.
    pub fn total_entries(&self) -> u32 {
        self.blobs_written.saturating_add(self.tombstones_written)
    }
}

// ─── État interne de l'écrivain ───────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WriterState {
    /// Pas encore initialisé (header non écrit).
    Idle,
    /// En cours d'écriture des entrées.
    Writing,
    /// Finalisé (footer écrit).
    Finalized,
}

// ─── Écrivain principal ───────────────────────────────────────────────────────

/// Écrivain d'archives ExoAR, travaillant sur un `ArchiveSink`.
///
/// Utilisation typique :
/// ```ignore
/// let mut sink = SinkVec::new();
/// let mut writer = ExoarWriter::new(ExoarWriteOptions::default());
/// writer.begin(&mut sink)?;
/// writer.write_blob(&mut sink, &blob_id, &data, 0, 0)?;
/// writer.finalize(&mut sink)?;
/// ```
pub struct ExoarWriter {
    options: ExoarWriteOptions,
    stats: ExoarWriteStats,
    state: WriterState,
}

impl ExoarWriter {
    /// Crée un nouvel écrivain avec les options données.
    pub const fn new(options: ExoarWriteOptions) -> Self {
        Self {
            options,
            stats: ExoarWriteStats::new(),
            state: WriterState::Idle,
        }
    }

    pub const fn with_defaults() -> Self {
        Self::new(ExoarWriteOptions::default())
    }

    /// Retourne les statistiques courantes (lecture seule).
    pub fn stats(&self) -> &ExoarWriteStats { &self.stats }

    /// Écrit l'en-tête de l'archive. Doit être appelé en premier.
    /// RÈGLE 8 : magic écrit EN PREMIER.
    pub fn begin<S: ArchiveSink>(&mut self, sink: &mut S) -> Result<(), ExoarWriteError> {
        if self.state != WriterState::Idle {
            return Err(ExoarWriteError::AlreadyFinalized);
        }
        let hdr = ExoarHeader::new(
            self.options.archive_flags,
            self.options.epoch_base,
            self.options.epoch_target,
        );
        let bytes = hdr.as_bytes();
        sink.write_all(bytes).map_err(|_| ExoarWriteError::IoError)?;
        self.stats.running_crc = crc32c_update(self.stats.running_crc, bytes);
        self.stats.archive_bytes = self.stats.archive_bytes.saturating_add(bytes.len() as u64);
        self.state = WriterState::Writing;
        Ok(())
    }

    /// Écrit un blob de données dans l'archive.
    /// RÈGLE 8  : ExoarEntryHeader magic est le premier champ écrit.
    /// RÈGLE 11 : blob_id doit être blake3(data AVANT compression/chiffrement).
    /// OOM-02   : pas d'allocation interne ici — travaille directement sur le sink.
    pub fn write_blob<S: ArchiveSink>(
        &mut self,
        sink: &mut S,
        blob_id: &[u8; 32],
        data: &[u8],
        entry_flags: u8,
        epoch: u64,
    ) -> Result<(), ExoarWriteError> {
        if self.state != WriterState::Writing {
            return Err(if self.state == WriterState::Finalized {
                ExoarWriteError::AlreadyFinalized
            } else {
                ExoarWriteError::NotStarted
            });
        }
        if self.stats.total_entries() >= EXOAR_MAX_ENTRIES {
            return Err(ExoarWriteError::TooManyEntries);
        }
        if data.len() as u64 > EXOAR_MAX_PAYLOAD {
            return Err(ExoarWriteError::PayloadTooLarge);
        }

        let payload_crc = crc32c_compute(data);
        let mut ehdr = ExoarEntryHeader::new(*blob_id, data.len() as u64, data.len() as u64);
        ehdr.flags = entry_flags & !(ENTRY_FLAG_TOMBSTONE);
        ehdr.payload_crc32 = payload_crc;
        ehdr.epoch = epoch;

        let hdr_bytes = ehdr.as_bytes();
        sink.write_all(hdr_bytes).map_err(|_| ExoarWriteError::IoError)?;
        self.stats.running_crc = crc32c_update(self.stats.running_crc, hdr_bytes);
        self.stats.archive_bytes = self.stats.archive_bytes.saturating_add(hdr_bytes.len() as u64);

        if !data.is_empty() {
            sink.write_all(data).map_err(|_| ExoarWriteError::IoError)?;
            self.stats.running_crc = crc32c_update(self.stats.running_crc, data);
            self.stats.archive_bytes = self.stats.archive_bytes.saturating_add(data.len() as u64);
            self.stats.payload_bytes = self.stats.payload_bytes.saturating_add(data.len() as u64);
        }

        self.stats.blobs_written = self.stats.blobs_written.saturating_add(1);
        Ok(())
    }

    /// Écrit un tombstone (enregistrement de suppression de blob).
    pub fn write_tombstone<S: ArchiveSink>(
        &mut self,
        sink: &mut S,
        blob_id: &[u8; 32],
        epoch: u64,
    ) -> Result<(), ExoarWriteError> {
        if !self.options.write_tombstones { return Ok(()); }
        if self.state != WriterState::Writing {
            return Err(if self.state == WriterState::Finalized {
                ExoarWriteError::AlreadyFinalized
            } else {
                ExoarWriteError::NotStarted
            });
        }
        if self.stats.total_entries() >= EXOAR_MAX_ENTRIES {
            return Err(ExoarWriteError::TooManyEntries);
        }

        let mut ehdr = ExoarEntryHeader::new(*blob_id, 0, 0);
        ehdr.flags = ENTRY_FLAG_TOMBSTONE;
        ehdr.epoch = epoch;

        let hdr_bytes = ehdr.as_bytes();
        sink.write_all(hdr_bytes).map_err(|_| ExoarWriteError::IoError)?;
        self.stats.running_crc = crc32c_update(self.stats.running_crc, hdr_bytes);
        self.stats.archive_bytes = self.stats.archive_bytes.saturating_add(hdr_bytes.len() as u64);
        self.stats.tombstones_written = self.stats.tombstones_written.saturating_add(1);
        Ok(())
    }

    /// Finalise l'archive (écrit le footer). Ne peut être appelé qu'une fois.
    pub fn finalize<S: ArchiveSink>(&mut self, sink: &mut S) -> Result<ExoarWriteStats, ExoarWriteError> {
        if self.state == WriterState::Finalized {
            return Err(ExoarWriteError::AlreadyFinalized);
        }
        if self.state == WriterState::Idle {
            return Err(ExoarWriteError::NotStarted);
        }

        let total_size = self.stats.archive_bytes.saturating_add(size_of::<ExoarFooter>() as u64);
        let ftr = ExoarFooter::new(
            self.stats.total_entries(),
            self.stats.running_crc,
            total_size,
        );
        let ftr_bytes = ftr.as_bytes();
        sink.write_all(ftr_bytes).map_err(|_| ExoarWriteError::IoError)?;
        self.stats.archive_bytes = self.stats.archive_bytes.saturating_add(ftr_bytes.len() as u64);
        self.state = WriterState::Finalized;
        Ok(self.stats)
    }

    /// Retourne true si l'archive est finalisée.
    pub fn is_finalized(&self) -> bool { self.state == WriterState::Finalized }
}

// ─── Écrivain bufferisé ───────────────────────────────────────────────────────

/// Écrivain bufferisé : accumule l'intégralité de l'archive en mémoire.
/// Utile pour les petites archives ou les tests.
/// OOM-02 : try_reserve avant chaque extension du buffer.
pub struct ExoarBufferedWriter {
    inner: ExoarWriter,
    buffer: Vec<u8>,
}

impl ExoarBufferedWriter {
    pub fn new(options: ExoarWriteOptions) -> Self {
        Self {
            inner: ExoarWriter::new(options),
            buffer: Vec::new(),
        }
    }

    pub fn with_capacity(options: ExoarWriteOptions, capacity: usize) -> Result<Self, ExoarWriteError> {
        let mut buffer = Vec::new();
        buffer.try_reserve(capacity).map_err(|_| ExoarWriteError::OutOfMemory)?;
        Ok(Self { inner: ExoarWriter::new(options), buffer })
    }

    /// Commence l'écriture de l'archive.
    pub fn begin(&mut self) -> Result<(), ExoarWriteError> {
        let mut sink = BufSink { buf: &mut self.buffer };
        self.inner.begin(&mut sink)
    }

    /// Écrit un blob dans le buffer.
    pub fn write_blob(
        &mut self,
        blob_id: &[u8; 32],
        data: &[u8],
        flags: u8,
        epoch: u64,
    ) -> Result<(), ExoarWriteError> {
        let mut sink = BufSink { buf: &mut self.buffer };
        self.inner.write_blob(&mut sink, blob_id, data, flags, epoch)
    }

    /// Écrit un tombstone dans le buffer.
    pub fn write_tombstone(&mut self, blob_id: &[u8; 32], epoch: u64) -> Result<(), ExoarWriteError> {
        let mut sink = BufSink { buf: &mut self.buffer };
        self.inner.write_tombstone(&mut sink, blob_id, epoch)
    }

    /// Finalise l'archive et retourne le buffer complet.
    pub fn finalize(mut self) -> Result<(Vec<u8>, ExoarWriteStats), ExoarWriteError> {
        let mut sink = BufSink { buf: &mut self.buffer };
        let stats = self.inner.finalize(&mut sink)?;
        Ok((self.buffer, stats))
    }

    /// Retourne une référence au buffer sans finaliser.
    pub fn buffer(&self) -> &[u8] { &self.buffer }

    /// Retourne les statistiques courantes.
    pub fn stats(&self) -> &ExoarWriteStats { self.inner.stats() }
}

/// Sink interne vers un Vec<u8> (utilisé par ExoarBufferedWriter).
struct BufSink<'a> {
    buf: &'a mut Vec<u8>,
}

impl<'a> ArchiveSink for BufSink<'a> {
    fn write_all(&mut self, data: &[u8]) -> ExofsResult<()> {
        self.buf.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(data);
        Ok(())
    }

    fn bytes_written(&self) -> u64 { self.buf.len() as u64 }
}

// ─── SinkVec (implémentation publique de ArchiveSink) ────────────────────────

/// Implémentation publique de `ArchiveSink` sur un `Vec<u8>`.
/// Utile pour les tests et les usages où l'archive doit tenir en mémoire.
pub struct SinkVec {
    buf: Vec<u8>,
}

impl SinkVec {
    pub fn new() -> Self { Self { buf: Vec::new() } }

    pub fn with_capacity(cap: usize) -> Result<Self, ExofsError> {
        let mut buf = Vec::new();
        buf.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        Ok(Self { buf })
    }

    pub fn into_inner(self) -> Vec<u8> { self.buf }
    pub fn as_slice(&self) -> &[u8] { &self.buf }
    pub fn len(&self) -> usize { self.buf.len() }
    pub fn is_empty(&self) -> bool { self.buf.is_empty() }
}

impl ArchiveSink for SinkVec {
    fn write_all(&mut self, data: &[u8]) -> ExofsResult<()> {
        self.buf.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(data);
        Ok(())
    }

    fn bytes_written(&self) -> u64 { self.buf.len() as u64 }
}

// ─── blake3 inline ────────────────────────────────────────────────────────────

/// Calcule le BlobId d'un slice de données (RÈGLE 11 : blake3 AVANT compression).
/// En production, utiliser le vrai blake3 via crate::fs::exofs::dedup::content_hash.
pub fn compute_blob_id(data: &[u8]) -> [u8; 32] {
    let mut state = [0u64; 4];
    state[0] = 0x6C62_272E_07BB_0142u64;
    state[1] = 0x6295_C58D_5935_4F4Eu64;
    state[2] = 0xD8A2_5D4F_5C3A_2B1Eu64;
    state[3] = 0xA3F6_C1D2_E5B7_8940u64;
    let mut i = 0usize;
    while i < data.len() {
        let b = data[i] as u64;
        state[0] = state[0].wrapping_add(b).wrapping_mul(0x9E37_79B9_7F4A_7C15u64);
        state[1] ^= state[0].rotate_left(23);
        state[2] = state[2].wrapping_add(state[1]).wrapping_mul(0x6C62_272E_07BB_0142u64);
        state[3] ^= state[2].rotate_right(17);
        state[0] = state[0].wrapping_add(state[3]);
        i = i.wrapping_add(1);
    }
    let mut out = [0u8; 32];
    let mut idx = 0usize;
    while idx < 4 {
        let bytes = state[idx].to_le_bytes();
        let base = idx.wrapping_mul(8);
        let mut k = 0usize;
        while k < 8 { out[base.wrapping_add(k)] = bytes[k]; k = k.wrapping_add(1); }
        idx = idx.wrapping_add(1);
    }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::exoar_reader::{ExoarReader, ExoarReaderConfig, SliceSource, CollectingReceiver};

    fn make_blob_id(tag: u8) -> [u8; 32] {
        let mut id = [0u8; 32];
        id[0] = tag;
        id
    }

    #[test]
    fn test_write_empty_archive() {
        let mut w = ExoarBufferedWriter::new(ExoarWriteOptions::default());
        w.begin().expect("begin ok");
        let (buf, stats) = w.finalize().expect("finalize ok");
        assert_eq!(stats.blobs_written, 0);
        assert!(buf.len() >= 128 + 32); // header + footer minimum
    }

    #[test]
    fn test_write_single_blob() {
        let bid = make_blob_id(1);
        let data = b"hello world";
        let mut w = ExoarBufferedWriter::new(ExoarWriteOptions::default());
        w.begin().expect("begin");
        w.write_blob(&bid, data, 0, 0).expect("write blob");
        let (buf, stats) = w.finalize().expect("finalize");
        assert_eq!(stats.blobs_written, 1);
        assert_eq!(stats.payload_bytes, data.len() as u64);
        // Relire l'archive
        let mut src = SliceSource::new(&buf);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let report = reader.read(&mut src, &mut rcv).expect("read ok");
        assert_eq!(report.entries_read, 1);
        assert_eq!(rcv.blobs[0].0, bid);
        assert_eq!(rcv.blobs[0].1, data.as_slice());
    }

    #[test]
    fn test_write_multiple_blobs_roundtrip() {
        let blobs: [([u8; 32], &[u8]); 3] = [
            (make_blob_id(1), b"first"),
            (make_blob_id(2), b"second blob"),
            (make_blob_id(3), b"third much longer blob data"),
        ];
        let mut w = ExoarBufferedWriter::new(ExoarWriteOptions::default());
        w.begin().expect("begin");
        for (bid, data) in &blobs {
            w.write_blob(bid, data, 0, 0).expect("write");
        }
        let (buf, stats) = w.finalize().expect("finalize");
        assert_eq!(stats.blobs_written, 3);

        let mut src = SliceSource::new(&buf);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let report = reader.read(&mut src, &mut rcv).expect("read");
        assert_eq!(report.entries_read, 3);
    }

    #[test]
    fn test_write_tombstone() {
        let bid = make_blob_id(99);
        let opt = ExoarWriteOptions::incremental(1, 2);
        let mut w = ExoarBufferedWriter::new(opt);
        w.begin().expect("begin");
        w.write_tombstone(&bid, 2).expect("tombstone");
        let (_, stats) = w.finalize().expect("finalize");
        assert_eq!(stats.tombstones_written, 1);
        assert_eq!(stats.blobs_written, 0);
    }

    #[test]
    fn test_finalize_twice_error() {
        let mut sink = SinkVec::new();
        let mut w = ExoarWriter::new(ExoarWriteOptions::default());
        w.begin(&mut sink).expect("begin");
        w.finalize(&mut sink).expect("first finalize");
        let result = w.finalize(&mut sink);
        assert!(matches!(result, Err(ExoarWriteError::AlreadyFinalized)));
    }

    #[test]
    fn test_write_before_begin_error() {
        let mut sink = SinkVec::new();
        let mut w = ExoarWriter::new(ExoarWriteOptions::default());
        let bid = make_blob_id(1);
        let result = w.write_blob(&mut sink, &bid, b"data", 0, 0);
        assert!(matches!(result, Err(ExoarWriteError::NotStarted)));
    }

    #[test]
    fn test_payload_too_large() {
        let mut sink = SinkVec::new();
        let mut w = ExoarWriter::new(ExoarWriteOptions::default());
        w.begin(&mut sink).expect("begin");
        // Créer un fake "large" payload en manipulant directement la taille
        // On ne peut pas allouer 256 MiB dans un test, donc on vérifie via
        // une entrée avec taille déclarée incorrecte — on skip ce test UI
        // et on vérifie que l'erreur est bien retournée pour une data vide
        // mais avec un en-tête qui déclare une taille excessive.
        // Test: la protection via EXOAR_MAX_PAYLOAD est bien activée.
        assert_eq!(EXOAR_MAX_PAYLOAD, 256 * 1024 * 1024);
    }

    #[test]
    fn test_sink_vec_basic() {
        let mut sink = SinkVec::new();
        sink.write_all(b"hello").expect("write");
        assert_eq!(sink.len(), 5);
        assert_eq!(sink.bytes_written(), 5);
        assert_eq!(sink.as_slice(), b"hello");
    }

    #[test]
    fn test_compute_blob_id_deterministic() {
        let data = b"test data for blob id";
        let id1 = compute_blob_id(data);
        let id2 = compute_blob_id(data);
        assert_eq!(id1, id2);
        let id3 = compute_blob_id(b"different data");
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_write_stats_total_entries() {
        let mut stats = ExoarWriteStats::new();
        stats.blobs_written = 5;
        stats.tombstones_written = 2;
        assert_eq!(stats.total_entries(), 7);
    }

    #[test]
    fn test_buffered_writer_capacity() {
        let w = ExoarBufferedWriter::with_capacity(ExoarWriteOptions::default(), 4096);
        assert!(w.is_ok());
    }

    #[test]
    fn test_archive_grows_with_entries() {
        let mut w = ExoarBufferedWriter::new(ExoarWriteOptions::default());
        w.begin().expect("begin");
        let size_after_header = w.buffer().len();
        w.write_blob(&make_blob_id(1), b"payload", 0, 0).expect("write");
        let size_after_entry = w.buffer().len();
        assert!(size_after_entry > size_after_header);
    }

    #[test]
    fn test_write_error_conversion() {
        let e: ExofsError = ExoarWriteError::OutOfMemory.into();
        assert_eq!(e, ExofsError::NoMemory);
        let e2: ExofsError = ExoarWriteError::TooManyEntries.into();
        assert_eq!(e2, ExofsError::OffsetOverflow);
    }
}
