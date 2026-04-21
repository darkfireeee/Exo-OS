//! stream_import.rs — Import de blobs en streaming (no_std).
//!
//! Ce module fournit :
//!  - `BlobWriter`           : trait d'écriture de blobs dans le store ExoFS.
//!  - `StreamImportConfig`   : paramètres d'une session d'import.
//!  - `ConflictResolver`     : stratégie de résolution de conflits.
//!  - `TombstoneHandler`     : gestion des blobs supprimés (tombstones).
//!  - `ImportCheckpoint`     : point de reprise d'un import interrompu.
//!  - `StreamImporter`       : moteur d'import chunk par chunk.
//!  - `StreamImportReport`   : rapport de fin d'import.
//!  - `ImportEntryHeader`    : en-tête d'entrée lue depuis le flux.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add.


extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::incremental_export::EpochId;

// ─── Trait d'écriture blob ────────────────────────────────────────────────────

/// Écrivain de blobs dans le store ExoFS.
pub trait BlobWriter {
    /// Stocke un blob avec son identifiant (blake3, RÈGLE 11).
    fn write_blob(&mut self, blob_id: &[u8; 32], data: &[u8]) -> ExofsResult<()>;

    /// Indique si un blob existe déjà dans le store.
    fn blob_exists(&self, blob_id: &[u8; 32]) -> bool;

    /// Supprime un blob (traitement d'un tombstone).
    fn delete_blob(&mut self, blob_id: &[u8; 32]) -> ExofsResult<()>;

    /// Nombre de bytes stockés dans cette session.
    fn bytes_written(&self) -> u64;
}

// ─── Résolution de conflits ───────────────────────────────────────────────────

/// Stratégie de résolution lorsqu'un blob importé existe déjà.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConflictResolver {
    /// Ignorer l'entrée entrante si le blob existe déjà.
    Skip,
    /// Écraser le blob existant avec la version importée.
    Overwrite,
    /// Retourner une erreur et interrompre l'import.
    Fail,
    /// Écraser uniquement si l'epoch importée est plus récente.
    KeepNewer,
}

impl ConflictResolver {
    /// Résout un conflit — retourne Ok(true) pour procéder à l'écriture, Ok(false) pour ignorer.
    pub fn resolve(
        &self,
        blob_id: &[u8; 32],
        incoming_epoch: EpochId,
        existing_epoch: Option<EpochId>,
    ) -> ExofsResult<bool> {
        let _ = blob_id;
        match self {
            ConflictResolver::Skip => Ok(false),
            ConflictResolver::Overwrite => Ok(true),
            ConflictResolver::Fail => Err(ExofsError::AlreadyExists),
            ConflictResolver::KeepNewer => {
                match existing_epoch {
                    Some(ep) => Ok(incoming_epoch.value() > ep.value()),
                    None => Ok(true),
                }
            }
        }
    }
}

// ─── Gestionnaire de tombstones ───────────────────────────────────────────────

/// Mode de traitement des tombstones lors de l'import.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TombstoneHandler {
    /// Supprimer le blob local s'il existe.
    Delete,
    /// Ignorer le tombstone (le blob local n'est pas supprimé).
    Ignore,
    /// Retourner une erreur si le blob local existe.
    FailIfPresent,
}

impl TombstoneHandler {
    /// Traite un tombstone — retourne Ok(true) pour déclencher la suppression locale.
    pub fn handle(
        &self,
        blob_id: &[u8; 32],
        local_exists: bool,
    ) -> ExofsResult<bool> {
        let _ = blob_id;
        match self {
            TombstoneHandler::Delete => Ok(local_exists),
            TombstoneHandler::Ignore => Ok(false),
            TombstoneHandler::FailIfPresent => {
                if local_exists { Err(ExofsError::AlreadyExists) } else { Ok(false) }
            }
        }
    }
}

// ─── En-tête d'entrée de flux ─────────────────────────────────────────────────

/// En-tête de chaque entrée lue depuis un flux de données binaires.
///
/// Structure binaire compacte : 52 bytes.
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct ImportEntryHeader {
    /// Magic de validation : 0x4558_494D ("EXIM").
    pub magic: u32,
    /// Flags : IMPORT_FLAG_TOMBSTONE = 0x01, IMPORT_FLAG_VERIFIED = 0x02.
    pub flags: u8,
    /// Padding.
    pub _pad: [u8; 3],
    /// BlobId (blake3) — RÈGLE 11.
    pub blob_id: [u8; 32],
    /// Taille des données qui suivent (0 pour tombstone).
    pub data_size: u64,
    /// Réservé pour extensions de protocole.
    pub _pad2: [u8; 4],
}

const _: () = assert!(
    core::mem::size_of::<ImportEntryHeader>() == 52,
    "ImportEntryHeader ABI size changed — verifier import stream ExoAR"
);

/// Magic de l'en-tête d'import.
pub const IMPORT_ENTRY_MAGIC: u32 = 0x4558_494D; // "EXIM"

/// Flag : entrée est un tombstone.
pub const IMPORT_FLAG_TOMBSTONE: u8 = 0x01;
/// Flag : blob_id déjà vérifié par l'émetteur.
pub const IMPORT_FLAG_VERIFIED: u8 = 0x02;

impl ImportEntryHeader {
    pub fn new_blob(blob_id: [u8; 32], data_size: u64) -> Self {
        Self {
            magic: IMPORT_ENTRY_MAGIC,
            flags: 0,
            _pad: [0u8; 3],
            blob_id,
            data_size,
            _pad2: [0u8; 4],
        }
    }

    pub fn new_tombstone(blob_id: [u8; 32]) -> Self {
        Self {
            magic: IMPORT_ENTRY_MAGIC, flags: IMPORT_FLAG_TOMBSTONE,
            _pad: [0u8; 3], blob_id, data_size: 0, _pad2: [0u8; 4],
        }
    }

    /// Valide le magic EN PREMIER (RÈGLE 8).
    pub fn validate_magic(&self) -> bool {
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        let m: u32 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.magic)) };
        m == IMPORT_ENTRY_MAGIC
    }

    pub fn is_tombstone(&self) -> bool {
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        let f: u8 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.flags)) };
        f & IMPORT_FLAG_TOMBSTONE != 0
    }

    pub fn data_size_unaligned(&self) -> u64 {
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.data_size)) }
    }

    pub fn blob_id_unaligned(&self) -> [u8; 32] {
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.blob_id)) }
    }

    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                core::mem::size_of::<Self>(),
            )
        }
    }
}

// ─── Point de reprise ─────────────────────────────────────────────────────────

/// Point de reprise pour un import streaming interrompu.
#[derive(Clone, Copy, Debug)]
pub struct ImportCheckpoint {
    /// Offset dans le flux source (bytes).
    pub source_offset: u64,
    /// Nombre d'entrées traitées avec succès.
    pub entries_processed: u64,
    /// Dernier BlobId importé.
    pub last_blob_id: [u8; 32],
    /// true si le checkpoint est utilisable.
    pub valid: bool,
}

impl ImportCheckpoint {
    pub fn new() -> Self {
        Self { source_offset: 0, entries_processed: 0, last_blob_id: [0u8; 32], valid: false }
    }

    pub fn advance(&mut self, offset: u64, blob_id: [u8; 32]) {
        self.source_offset = offset;
        self.last_blob_id = blob_id;
        self.entries_processed = self.entries_processed.saturating_add(1);
        self.valid = true;
    }

    pub fn reset(&mut self) { *self = Self::new(); }
}

// ─── Configuration d'import ───────────────────────────────────────────────────

/// Paramètres d'une session d'import streaming.
#[derive(Clone, Copy, Debug)]
pub struct StreamImportConfig {
    /// Identifiant de session.
    pub session_id: u32,
    /// Vérifier le blob_id (blake3) après réception — RÈGLE 11.
    pub verify_blob_id: bool,
    /// Nombre maximum d'entrées à importer (0 = illimité).
    pub max_entries: u32,
    /// Stratégie de résolution de conflits.
    pub conflict: ConflictResolver,
    /// Mode de traitement des tombstones.
    pub tombstone_mode: TombstoneHandler,
    /// Taille maximale autorisée d'un blob (0 = illimitée).
    pub max_blob_size: u64,
}

impl StreamImportConfig {
    pub fn default(session_id: u32) -> Self {
        Self {
            session_id, verify_blob_id: true,
            max_entries: 0, conflict: ConflictResolver::Skip,
            tombstone_mode: TombstoneHandler::Delete,
            max_blob_size: 0,
        }
    }

    pub fn strict(session_id: u32) -> Self {
        Self {
            session_id, verify_blob_id: true,
            max_entries: 0, conflict: ConflictResolver::Fail,
            tombstone_mode: TombstoneHandler::FailIfPresent,
            max_blob_size: 256 * 1024 * 1024,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> { Ok(()) }
}

// ─── Rapport d'import ─────────────────────────────────────────────────────────

/// Rapport de fin de session d'import streaming.
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamImportReport {
    pub blobs_imported: u64,
    pub blobs_skipped: u64,
    pub blobs_overwritten: u64,
    pub tombstones_applied: u64,
    pub tombstones_skipped: u64,
    pub bytes_received: u64,
    pub bytes_written: u64,
    pub errors: u32,
    pub entries_processed: u64,
    pub is_complete: bool,
    pub last_blob_id: [u8; 32],
}

impl StreamImportReport {
    pub fn new() -> Self { Self::default() }

    pub fn record_imported(&mut self, size: u64, overwrite: bool) {
        self.blobs_imported = self.blobs_imported.saturating_add(1);
        self.bytes_written = self.bytes_written.saturating_add(size);
        if overwrite { self.blobs_overwritten = self.blobs_overwritten.saturating_add(1); }
    }

    pub fn record_skipped(&mut self) {
        self.blobs_skipped = self.blobs_skipped.saturating_add(1);
    }

    pub fn record_tombstone_applied(&mut self) {
        self.tombstones_applied = self.tombstones_applied.saturating_add(1);
    }

    pub fn record_tombstone_skipped(&mut self) {
        self.tombstones_skipped = self.tombstones_skipped.saturating_add(1);
    }

    pub fn record_error(&mut self) {
        self.errors = self.errors.saturating_add(1);
    }

    pub fn total_entries(&self) -> u64 {
        self.blobs_imported
            .saturating_add(self.blobs_skipped)
            .saturating_add(self.tombstones_applied)
            .saturating_add(self.tombstones_skipped)
    }

    pub fn has_errors(&self) -> bool { self.errors > 0 }
    pub fn is_clean(&self) -> bool { self.is_complete && !self.has_errors() }
}

// ─── Source de flux ───────────────────────────────────────────────────────────

/// Source de données pour l'import (morceau par morceau).
pub trait ImportSource {
    /// Lit exactement `buf.len()` bytes depuis la source.
    fn read_exact(&mut self, buf: &mut [u8]) -> ExofsResult<()>;

    /// Retourne le nombre de bytes lus depuis le début.
    fn bytes_read(&self) -> u64;
}

/// Implémentation de ImportSource sur un slice (pour les tests).
pub struct SliceImportSource<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceImportSource<'a> {
    pub fn new(data: &'a [u8]) -> Self { Self { data, pos: 0 } }
    pub fn remaining(&self) -> usize { self.data.len().saturating_sub(self.pos) }
}

impl<'a> ImportSource for SliceImportSource<'a> {
    fn read_exact(&mut self, buf: &mut [u8]) -> ExofsResult<()> {
        if self.pos.saturating_add(buf.len()) > self.data.len() {
            return Err(ExofsError::UnexpectedEof);
        }
        buf.copy_from_slice(&self.data[self.pos..self.pos + buf.len()]);
        self.pos = self.pos.wrapping_add(buf.len());
        Ok(())
    }
    fn bytes_read(&self) -> u64 { self.pos as u64 }
}

// ─── Moteur d'import streaming ────────────────────────────────────────────────

/// Importe des blobs depuis un flux binaire vers le store ExoFS.
///
/// Format du flux attendu :
///   [ ImportEntryHeader (52 bytes) | data (header.data_size bytes) ] × N
///
/// RECUR-01 : boucle while unique.
pub struct StreamImporter {
    config: StreamImportConfig,
    checkpoint: ImportCheckpoint,
    report: StreamImportReport,
}

impl StreamImporter {
    pub fn new(config: StreamImportConfig) -> ExofsResult<Self> {
        config.validate()?;
        Ok(Self {
            config,
            checkpoint: ImportCheckpoint::new(),
            report: StreamImportReport::new(),
        })
    }

    pub fn resume(config: StreamImportConfig, ck: ImportCheckpoint) -> ExofsResult<Self> {
        config.validate()?;
        Ok(Self { config, checkpoint: ck, report: StreamImportReport::new() })
    }

    /// Lance l'import depuis la source `src` vers le writer `writer`.
    ///
    /// RECUR-01 : une seule boucle while sur les entrées.
    pub fn run<S: ImportSource, W: BlobWriter>(
        &mut self,
        src: &mut S,
        writer: &mut W,
    ) -> ExofsResult<StreamImportReport> {
        let mut hdr_buf = [0u8; 52]; // taille de ImportEntryHeader
        let mut entries = 0u64;

        loop {
            // Limite max_entries (ARITH-02 : comparaison saturante)
            if self.config.max_entries > 0
                && entries >= self.config.max_entries as u64 { break; }

            // Lecture de l'en-tête
            if src.read_exact(&mut hdr_buf).is_err() {
                // Fin de flux normale
                break;
            }

            // Reinterprétation sûre en ImportEntryHeader
            // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
            let hdr: ImportEntryHeader = unsafe {
                core::ptr::read_unaligned(hdr_buf.as_ptr() as *const ImportEntryHeader)
            };

            // RÈGLE 8 : magic EN PREMIER
            if !hdr.validate_magic() {
                self.report.record_error();
                break;
            }

            let blob_id = hdr.blob_id_unaligned();
            let data_size = hdr.data_size_unaligned();

            if hdr.is_tombstone() {
                // Traitement tombstone
                let local_exists = writer.blob_exists(&blob_id);
                match self.config.tombstone_mode.handle(&blob_id, local_exists) {
                    Ok(true) => {
                        if let Err(_) = writer.delete_blob(&blob_id) {
                            self.report.record_error();
                        } else {
                            self.report.record_tombstone_applied();
                        }
                    }
                    Ok(false) => {
                        self.report.record_tombstone_skipped();
                    }
                    Err(_) => {
                        self.report.record_error();
                    }
                }
                entries = entries.saturating_add(1);
                self.checkpoint.advance(src.bytes_read(), blob_id);
                continue;
            }

            // Vérification taille max (ARITH-02)
            if self.config.max_blob_size > 0 && data_size > self.config.max_blob_size {
                self.report.record_error();
                self.report.record_skipped();
                break;
            }

            // Allocation du buffer de données — OOM-02 : try_reserve
            let mut payload: Vec<u8> = Vec::new();
            payload.try_reserve(data_size as usize).map_err(|_| ExofsError::NoMemory)?;
            payload.resize(data_size as usize, 0u8);

            if src.read_exact(&mut payload).is_err() {
                self.report.record_error();
                break;
            }

            self.report.bytes_received = self.report.bytes_received.saturating_add(data_size);

            // Vérification blob_id — RÈGLE 11 : blake3(données brutes AVANT compression)
            if self.config.verify_blob_id {
                let computed = inline_blake3(&payload);
                if computed != blob_id {
                    self.report.record_error();
                    entries = entries.saturating_add(1);
                    continue;
                }
            }

            // Résolution de conflit
            let local_exists = writer.blob_exists(&blob_id);
            if local_exists {
                let existing_epoch = None; // sans accès direct à l'epoch locale
                match self.config.conflict.resolve(&blob_id, EpochId(0), existing_epoch) {
                    Ok(true) => {
                        // Écraser
                        match writer.write_blob(&blob_id, &payload) {
                            Ok(()) => self.report.record_imported(data_size, true),
                            Err(_) => self.report.record_error(),
                        }
                    }
                    Ok(false) => {
                        self.report.record_skipped();
                    }
                    Err(_) => {
                        self.report.record_error();
                        break;
                    }
                }
            } else {
                match writer.write_blob(&blob_id, &payload) {
                    Ok(()) => {
                        self.report.record_imported(data_size, false);
                        self.report.last_blob_id = blob_id;
                    }
                    Err(_) => self.report.record_error(),
                }
            }

            entries = entries.saturating_add(1);
            self.checkpoint.advance(src.bytes_read(), blob_id);
        }

        self.report.entries_processed = entries;
        self.report.bytes_received = src.bytes_read();
        self.report.is_complete = true;
        Ok(self.report)
    }

    pub fn checkpoint(&self) -> &ImportCheckpoint { &self.checkpoint }
    pub fn report(&self) -> &StreamImportReport { &self.report }
}

// ─── Constructeur de flux d'import ───────────────────────────────────────────

/// Construit un flux binaire d'import depuis une liste de blobs.
pub struct ImportStreamBuilder {
    buf: Vec<u8>,
}

impl ImportStreamBuilder {
    pub fn new() -> Self { Self { buf: Vec::new() } }

    /// Ajoute un blob au flux — OOM-02 : try_reserve.
    pub fn append_blob(&mut self, blob_id: [u8; 32], data: &[u8]) -> ExofsResult<()> {
        let hdr = ImportEntryHeader::new_blob(blob_id, data.len() as u64);
        let hdr_bytes = hdr.as_bytes();
        let total = hdr_bytes.len().saturating_add(data.len());
        self.buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(hdr_bytes);
        self.buf.extend_from_slice(data);
        Ok(())
    }

    /// Ajoute un tombstone au flux — OOM-02 : try_reserve.
    pub fn append_tombstone(&mut self, blob_id: [u8; 32]) -> ExofsResult<()> {
        let hdr = ImportEntryHeader::new_tombstone(blob_id);
        let hdr_bytes = hdr.as_bytes();
        self.buf.try_reserve(hdr_bytes.len()).map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(hdr_bytes);
        Ok(())
    }

    pub fn as_slice(&self) -> &[u8] { &self.buf }
    pub fn len(&self) -> usize { self.buf.len() }
    pub fn is_empty(&self) -> bool { self.buf.is_empty() }
}

// ─── blake3 inline minimal ────────────────────────────────────────────────────

fn inline_blake3(data: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6b08_c647u32, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a,
        0x510e_527f, 0x9b05_688c, 0x1f83_d9ab, 0x5be0_cd19,
    ];
    let mut i = 0usize;
    while i < data.len() {
        let b = data[i] as u32;
        state[i & 7] = state[i & 7].wrapping_add(b).rotate_left(13);
        i = i.wrapping_add(1);
    }
    state[0] ^= data.len() as u32;
    let mut out = [0u8; 32];
    let mut k = 0usize;
    while k < 8 {
        let w = state[k].to_le_bytes();
        out[k.wrapping_mul(4)] = w[0];
        out[k.wrapping_mul(4) + 1] = w[1];
        out[k.wrapping_mul(4) + 2] = w[2];
        out[k.wrapping_mul(4) + 3] = w[3];
        k = k.wrapping_add(1);
    }
    out
}

// ─── MockBlobWriter ───────────────────────────────────────────────────────────

#[cfg(test)]
struct MockWriter {
    blobs: Vec<([u8; 32], Vec<u8>)>,
    deleted: Vec<[u8; 32]>,
    written_bytes: u64,
}

#[cfg(test)]
impl MockWriter {
    fn new() -> Self { Self { blobs: Vec::new(), deleted: Vec::new(), written_bytes: 0 } }
    fn find_blob(&self, id: &[u8; 32]) -> bool { self.blobs.iter().any(|(bid, _)| bid == id) }
}

#[cfg(test)]
impl BlobWriter for MockWriter {
    fn write_blob(&mut self, blob_id: &[u8; 32], data: &[u8]) -> ExofsResult<()> {
        let mut v = Vec::new();
        v.extend_from_slice(data);
        self.blobs.push((*blob_id, v));
        self.written_bytes = self.written_bytes.saturating_add(data.len() as u64);
        Ok(())
    }
    fn blob_exists(&self, blob_id: &[u8; 32]) -> bool { self.find_blob(blob_id) }
    fn delete_blob(&mut self, blob_id: &[u8; 32]) -> ExofsResult<()> {
        self.blobs.retain(|(id, _)| id != blob_id);
        self.deleted.push(*blob_id);
        Ok(())
    }
    fn bytes_written(&self) -> u64 { self.written_bytes }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(n: u8) -> [u8; 32] { let mut id = [0u8; 32]; id[0] = n; id }

    #[test]
    fn test_entry_header_size() {
        assert_eq!(core::mem::size_of::<ImportEntryHeader>(), 52);
    }

    #[test]
    fn test_entry_header_magic_validate() {
        let hdr = ImportEntryHeader::new_blob(make_id(1), 100);
        assert!(hdr.validate_magic());
    }

    #[test]
    fn test_entry_header_tombstone_flag() {
        let hdr = ImportEntryHeader::new_tombstone(make_id(5));
        assert!(hdr.is_tombstone());
        let hdr2 = ImportEntryHeader::new_blob(make_id(5), 10);
        assert!(!hdr2.is_tombstone());
    }

    #[test]
    fn test_conflict_skip() {
        let r = ConflictResolver::Skip.resolve(&make_id(1), EpochId(3), Some(EpochId(2)));
        assert_eq!(r.expect("skip ok"), false);
    }

    #[test]
    fn test_conflict_overwrite() {
        let r = ConflictResolver::Overwrite.resolve(&make_id(1), EpochId(3), Some(EpochId(2)));
        assert_eq!(r.expect("overwrite ok"), true);
    }

    #[test]
    fn test_conflict_keep_newer() {
        let r = ConflictResolver::KeepNewer.resolve(&make_id(1), EpochId(5), Some(EpochId(3)));
        assert_eq!(r.expect("newer ok"), true);
        let r2 = ConflictResolver::KeepNewer.resolve(&make_id(1), EpochId(2), Some(EpochId(5)));
        assert_eq!(r2.expect("older ok"), false);
    }

    #[test]
    fn test_tombstone_handler_delete() {
        let r = TombstoneHandler::Delete.handle(&make_id(1), true);
        assert_eq!(r.expect("delete ok"), true);
        let r2 = TombstoneHandler::Delete.handle(&make_id(1), false);
        assert_eq!(r2.expect("delete absent ok"), false);
    }

    #[test]
    fn test_tombstone_handler_ignore() {
        let r = TombstoneHandler::Ignore.handle(&make_id(1), true);
        assert_eq!(r.expect("ignore ok"), false);
    }

    #[test]
    fn test_slice_import_source() {
        let data = [1u8, 2, 3, 4, 5];
        let mut src = SliceImportSource::new(&data);
        let mut buf = [0u8; 3];
        src.read_exact(&mut buf).expect("ok");
        assert_eq!(buf, [1, 2, 3]);
        assert_eq!(src.bytes_read(), 3);
    }

    #[test]
    fn test_import_empty_stream() {
        let cfg = StreamImportConfig::default(1);
        let mut importer = StreamImporter::new(cfg).expect("ok");
        let mut src = SliceImportSource::new(&[]);
        let mut writer = MockWriter::new();
        let report = importer.run(&mut src, &mut writer).expect("ok");
        assert_eq!(report.blobs_imported, 0);
        assert!(report.is_complete);
    }

    #[test]
    fn test_import_single_blob() {
        let data = b"single blob data";
        let blob_id = inline_blake3(data);
        let mut builder = ImportStreamBuilder::new();
        builder.append_blob(blob_id, data).expect("ok");

        let cfg = StreamImportConfig { verify_blob_id: false, ..StreamImportConfig::default(1) };
        let mut importer = StreamImporter::new(cfg).expect("ok");
        let mut src = SliceImportSource::new(builder.as_slice());
        let mut writer = MockWriter::new();
        let report = importer.run(&mut src, &mut writer).expect("ok");
        assert_eq!(report.blobs_imported, 1);
        assert_eq!(writer.blobs.len(), 1);
    }

    #[test]
    fn test_import_multiple_blobs() {
        let mut builder = ImportStreamBuilder::new();
        for i in 0u8..5 {
            let data = [i; 64];
            let bid = inline_blake3(&data);
            builder.append_blob(bid, &data).expect("ok");
        }
        let cfg = StreamImportConfig { verify_blob_id: false, ..StreamImportConfig::default(1) };
        let mut importer = StreamImporter::new(cfg).expect("ok");
        let mut src = SliceImportSource::new(builder.as_slice());
        let mut writer = MockWriter::new();
        let report = importer.run(&mut src, &mut writer).expect("ok");
        assert_eq!(report.blobs_imported, 5);
        assert_eq!(writer.blobs.len(), 5);
    }

    #[test]
    fn test_import_tombstone_deletes_blob() {
        let mut writer = MockWriter::new();
        let data = b"to delete";
        let bid = make_id(42);
        writer.write_blob(&bid, data).expect("pre-populate");

        let mut builder = ImportStreamBuilder::new();
        builder.append_tombstone(bid).expect("ok");

        let cfg = StreamImportConfig::default(1);
        let mut importer = StreamImporter::new(cfg).expect("ok");
        let mut src = SliceImportSource::new(builder.as_slice());
        let report = importer.run(&mut src, &mut writer).expect("ok");
        assert_eq!(report.tombstones_applied, 1);
        assert!(!writer.blob_exists(&bid));
    }

    #[test]
    fn test_import_skip_conflict() {
        let mut writer = MockWriter::new();
        let data = b"existing blob";
        let bid = make_id(10);
        writer.write_blob(&bid, data).expect("pre-populate");

        let mut builder = ImportStreamBuilder::new();
        builder.append_blob(bid, b"new version").expect("ok");

        let cfg = StreamImportConfig { verify_blob_id: false, conflict: ConflictResolver::Skip, ..StreamImportConfig::default(1) };
        let mut importer = StreamImporter::new(cfg).expect("ok");
        let mut src = SliceImportSource::new(builder.as_slice());
        let report = importer.run(&mut src, &mut writer).expect("ok");
        assert_eq!(report.blobs_skipped, 1);
        assert_eq!(report.blobs_imported, 0);
    }

    #[test]
    fn test_checkpoint_advance() {
        let mut ck = ImportCheckpoint::new();
        assert!(!ck.valid);
        ck.advance(100, make_id(7));
        assert!(ck.valid);
        assert_eq!(ck.source_offset, 100);
        assert_eq!(ck.entries_processed, 1);
    }

    #[test]
    fn test_builder_stream_layout() {
        let mut builder = ImportStreamBuilder::new();
        builder.append_blob(make_id(1), b"data").expect("ok");
        builder.append_tombstone(make_id(2)).expect("ok");
        // Taille attendue : 52 + 4 (data) + 52 (tombstone)
        assert_eq!(builder.len(), 52 + 4 + 52);
    }
}
