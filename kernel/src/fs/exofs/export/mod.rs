//! mod.rs — Module public d'export ExoFS.
//!
//! Ce module est le point d'entrée de toutes les capacités d'export/import ExoFS.
//!
//! Il fournit :
//!  - `pub mod` + `pub use` re-exports de tous les types publics.
//!  - `ExportConfig`          : configuration globale du module export.
//!  - `ExportModule`          : façade de haut niveau pour export/import.
//!  - `ExportHealthStatus`    : état de santé du module.
//!  - `ExportModuleSummary`   : résumé des opérations menées.
//!  - Fonctions de commodité  : `export_blob_to_archive`, `import_from_archive`, `verify_archive`.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add.

// ─── Sous-modules ─────────────────────────────────────────────────────────────
pub mod exoar_format;
pub mod exoar_reader;
pub mod exoar_writer;
pub mod export_audit;
pub mod incremental_export;
pub mod metadata_export;
pub mod stream_export;
pub mod stream_import;
pub mod tar_compat;

// ─── Re-exports publics ───────────────────────────────────────────────────────

// Format ExoAR
pub use exoar_format::{
    crc32c_compute, crc32c_verify, ExoarEntryHeader, ExoarEntryInfo, ExoarEntryKind, ExoarFooter,
    ExoarHeader, ExoarSummary, ARCHIVE_FLAG_INCREMENTAL, ARCHIVE_FLAG_SNAPSHOT,
    ARCHIVE_FLAG_VERIFIED, ENTRY_FLAG_COMPRESSED, ENTRY_FLAG_ENCRYPTED, ENTRY_FLAG_TOMBSTONE,
    EXOAR_ENTRY_MAGIC, EXOAR_FOOTER_MAGIC, EXOAR_MAGIC, EXOAR_MAX_ENTRIES, EXOAR_MAX_PAYLOAD,
    EXOAR_VERSION,
};

// Lecture ExoAR
pub use exoar_reader::{
    ArchiveSource, BlobReceiver, CollectingReceiver, ExoarReadError, ExoarReadReport, ExoarReader,
    ExoarReaderConfig, ExoarScanner, SliceSource,
};

// Écriture ExoAR
pub use exoar_writer::{
    ArchiveSink, ExoarBufferedWriter, ExoarWriteError, ExoarWriteOptions, ExoarWriteStats,
    ExoarWriter, SinkVec,
};

// Audit
pub use export_audit::{
    ExportAuditEntry, ExportAuditLog, ExportAuditStats, ExportEvent, ExportSession,
    ExportSessionConfig, SessionState, EXPORT_AUDIT, EXPORT_AUDIT_RING,
};

// Export incrémental
pub use incremental_export::{
    DiffSet, EpochId, EpochRange, IncrementalBlobSource, IncrementalExport,
    IncrementalExportConfig, IncrementalExportResult, MultiEpochExportSummary, SnapshotExport,
};

// Métadonnées
pub use metadata_export::{
    BlobMeta, ChunkMeta, ExportManifest, ManifestBlobEntry, MetaBinaryHeader, MetadataBinaryWriter,
    MetadataExporter, SnapshotMeta, TextSink, VecTextSink, META_BINARY_MAGIC,
};

// Export streaming
pub use stream_export::{
    BatchState, BlobDataProvider, FilterMode, StreamCheckpoint, StreamExportBatch,
    StreamExportConfig, StreamExportReport, StreamExporter, StreamFilter, StreamSink,
};

// Import streaming
pub use stream_import::{
    BlobWriter, ConflictResolver, ImportCheckpoint, ImportEntryHeader, ImportSource,
    ImportStreamBuilder, SliceImportSource, StreamImportConfig, StreamImportReport, StreamImporter,
    TombstoneHandler, IMPORT_ENTRY_MAGIC, IMPORT_FLAG_TOMBSTONE, IMPORT_FLAG_VERIFIED,
};

// Compatibilité tar
pub use tar_compat::{
    tar_checksum_compute, tar_checksum_verify, ExoarToTarConverter, SliceTarSource, TarBlock,
    TarEmitStats, TarEmitter, TarEntry, TarEntryKind, TarHeader, TarParseReport, TarParser,
    TarSink, TarSource, TarToExoarConverter, TarToExoarResult, VecTarSink, TAR_BLOCK_SIZE,
    TAR_MAGIC, TAR_NAME_MAX, TAR_VERSION,
};

// ─── Erreurs et résultats kernel ─────────────────────────────────────────────
use crate::fs::exofs::core::{ExofsError, ExofsResult};

extern crate alloc;
use alloc::vec::Vec;

// ─── Configuration globale du module export ──────────────────────────────────

/// Configuration globale du module d'export ExoFS.
#[derive(Clone, Copy, Debug)]
pub struct ExportConfig {
    /// Identifiant de session (incrémenté à chaque session).
    pub session_id: u32,
    /// Activer l'audit des opérations d'export/import.
    pub audit_enabled: bool,
    /// Vérifier les blob_ids (RÈGLE 11) lors de l'export.
    pub verify_blob_ids: bool,
    /// Taille de bloc pour les transferts streaming.
    pub block_size: u32,
    /// Nombre maximum d'entrées par opération (0 = illimité).
    pub max_entries: u32,
    /// Générer également un manifest texte ASCII à côté de l'archive.
    pub generate_manifest: bool,
    /// Stratégie de résolution de conflits lors de l'import.
    pub conflict: ConflictResolver,
    /// Mode de traitement des tombstones lors de l'import.
    pub tombstone_mode: TombstoneHandler,
}

impl ExportConfig {
    /// Configuration par défaut — vérification activée, audit activé.
    pub fn default_config(session_id: u32) -> Self {
        Self {
            session_id,
            audit_enabled: true,
            verify_blob_ids: true,
            block_size: 65536,
            max_entries: 0,
            generate_manifest: true,
            conflict: ConflictResolver::Skip,
            tombstone_mode: TombstoneHandler::Delete,
        }
    }

    /// Configuration stricte — toute erreur interrompt l'opération.
    pub fn strict(session_id: u32) -> Self {
        Self {
            session_id,
            audit_enabled: true,
            verify_blob_ids: true,
            block_size: 65536,
            max_entries: 0,
            generate_manifest: true,
            conflict: ConflictResolver::Fail,
            tombstone_mode: TombstoneHandler::FailIfPresent,
        }
    }

    /// Configuration rapide — pas de vérification blob_id (performance).
    pub fn fast(session_id: u32) -> Self {
        Self {
            session_id,
            audit_enabled: false,
            verify_blob_ids: false,
            block_size: 131072,
            max_entries: 0,
            generate_manifest: false,
            conflict: ConflictResolver::Skip,
            tombstone_mode: TombstoneHandler::Delete,
        }
    }

    /// Retourne les options d'écriture ExoAR dérivées de la config.
    pub fn write_options(&self) -> ExoarWriteOptions {
        ExoarWriteOptions::default()
    }

    /// Retourne la config d'import streaming dérivée.
    pub fn import_config(&self) -> StreamImportConfig {
        StreamImportConfig {
            session_id: self.session_id,
            verify_blob_id: self.verify_blob_ids,
            max_entries: self.max_entries,
            conflict: self.conflict,
            tombstone_mode: self.tombstone_mode,
            max_blob_size: 0,
        }
    }

    /// Retourne la config d'export streaming dérivée.
    pub fn stream_export_config(&self) -> StreamExportConfig {
        StreamExportConfig {
            session_id: self.session_id,
            verify_blob_id: self.verify_blob_ids,
            block_size: self.block_size,
            max_blobs: self.max_entries,
            include_tombstones: true,
            epoch_min: 0,
            epoch_max: u64::MAX,
        }
    }
}

// ─── État de santé du module ──────────────────────────────────────────────────

/// État de santé du module d'export.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExportHealthStatus {
    /// Module opérationnel, aucune erreur.
    Healthy,
    /// Module opérationnel, quelques erreurs non critiques.
    Degraded,
    /// Module défaillant, trop d'erreurs récentes.
    Failed,
}

impl ExportHealthStatus {
    pub fn is_operational(&self) -> bool {
        !matches!(self, ExportHealthStatus::Failed)
    }
}

// ─── Résumé des opérations ────────────────────────────────────────────────────

/// Résumé cumulatif des opérations d'export/import.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExportModuleSummary {
    pub total_exports: u64,
    pub total_imports: u64,
    pub total_blobs_exported: u64,
    pub total_blobs_imported: u64,
    pub total_bytes_exported: u64,
    pub total_bytes_imported: u64,
    pub total_errors: u32,
    pub total_sessions: u32,
}

impl ExportModuleSummary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_export(&mut self, blobs: u64, bytes: u64) {
        self.total_exports = self.total_exports.saturating_add(1);
        self.total_blobs_exported = self.total_blobs_exported.saturating_add(blobs);
        self.total_bytes_exported = self.total_bytes_exported.saturating_add(bytes);
        self.total_sessions = self.total_sessions.saturating_add(1);
    }

    pub fn record_import(&mut self, blobs: u64, bytes: u64) {
        self.total_imports = self.total_imports.saturating_add(1);
        self.total_blobs_imported = self.total_blobs_imported.saturating_add(blobs);
        self.total_bytes_imported = self.total_bytes_imported.saturating_add(bytes);
        self.total_sessions = self.total_sessions.saturating_add(1);
    }

    pub fn record_error(&mut self) {
        self.total_errors = self.total_errors.saturating_add(1);
    }

    pub fn is_clean(&self) -> bool {
        self.total_errors == 0
    }
}

// ─── Façade du module export ──────────────────────────────────────────────────

/// Façade de haut niveau pour les opérations d'export/import ExoFS.
///
/// `ExportModule` est le point d'entrée principal côté kernel.
/// Il orchestre toutes les couches sous-jacentes.
pub struct ExportModule {
    config: ExportConfig,
    summary: ExportModuleSummary,
}

impl ExportModule {
    /// Initialise le module d'export avec la configuration donnée.
    pub fn init(config: ExportConfig) -> Self {
        Self {
            config,
            summary: ExportModuleSummary::new(),
        }
    }

    /// Exporte un snapshot complet des blobs vers un `ExoarBufferedWriter`.
    ///
    /// `blob_ids` : liste des BlobId à exporter.
    /// `provider` : source de données blob.
    ///
    /// Retourne le buffer de l'archive produite.
    pub fn export_snapshot<P: BlobDataProvider>(
        &mut self,
        provider: &P,
        blob_ids: &[[u8; 32]],
    ) -> ExofsResult<Vec<u8>> {
        if self.config.audit_enabled {
            EXPORT_AUDIT.log_event(0, ExportEvent::SessionStarted);
        }

        let opts = ExoarWriteOptions::snapshot(0);
        let mut writer = ExoarBufferedWriter::new(opts);
        writer.begin()?;

        let block = self.config.block_size as usize;
        let mut i = 0usize;

        // RECUR-01 : boucle while
        while i < blob_ids.len() {
            let blob_id = &blob_ids[i];
            match provider.provide_blob(blob_id) {
                Ok(data) => {
                    writer.write_blob(blob_id, data, 0, 0)?;
                    if self.config.audit_enabled {
                        EXPORT_AUDIT.log_blob_exported(0, blob_id, data.len() as u64, 0);
                    }
                    self.summary.total_blobs_exported =
                        self.summary.total_blobs_exported.saturating_add(1);
                    let _ = block;
                }
                Err(_) => {
                    self.summary.record_error();
                }
            }
            i = i.wrapping_add(1);
        }

        let (buf, _) = writer.finalize()?;

        self.summary
            .record_export(self.summary.total_blobs_exported, buf.len() as u64);

        if self.config.audit_enabled {
            EXPORT_AUDIT.log_event(0, ExportEvent::SessionCompleted);
        }
        Ok(buf)
    }

    /// Export incrémental entre deux epochs.
    pub fn export_incremental<P: BlobDataProvider>(
        &mut self,
        provider: &P,
        added_blobs: &[[u8; 32]],
        deleted_blobs: &[[u8; 32]],
    ) -> ExofsResult<Vec<u8>> {
        if self.config.audit_enabled {
            EXPORT_AUDIT.log_event(0, ExportEvent::IncrementalExport);
        }

        let opts = ExoarWriteOptions::incremental(0, 0);
        let mut writer = ExoarBufferedWriter::new(opts);
        writer.begin()?;

        // Phase 1 : blobs ajoutés (RECUR-01 : boucle while)
        let mut i = 0usize;
        while i < added_blobs.len() {
            let blob_id = &added_blobs[i];
            if let Ok(data) = provider.provide_blob(blob_id) {
                writer.write_blob(blob_id, data, 0, 0)?;
            }
            i = i.wrapping_add(1);
        }

        // Phase 2 : tombstones (RECUR-01 : boucle while)
        let mut j = 0usize;
        while j < deleted_blobs.len() {
            writer.write_tombstone(&deleted_blobs[j], 0)?;
            j = j.wrapping_add(1);
        }

        let (buf, _) = writer.finalize()?;
        self.summary
            .record_export(added_blobs.len() as u64, buf.len() as u64);
        Ok(buf)
    }

    /// Importe des blobs depuis une archive ExoAR en mémoire.
    pub fn import<W: BlobWriter>(
        &mut self,
        archive: &[u8],
        writer: &mut W,
    ) -> ExofsResult<StreamImportReport> {
        if self.config.audit_enabled {
            EXPORT_AUDIT.log_event(0, ExportEvent::SessionStarted);
        }

        // Utiliser le StreamImporter avec l'archive comme source
        let mut builder = ImportStreamBuilder::new();

        // Lire l'archive via ExoarReader pour extraire les blobs puis les ré-encoder
        // en flux import. Implémentation directe via CollectingReceiver.
        let cfg = self.config.import_config();
        let mut importer = StreamImporter::new(cfg)?;

        // Conversion archive ExoAR → flux import binaire (RECUR-01 : boucle while)
        {
            let config = ExoarReaderConfig::default();
            let mut receiver = CollectingReceiver::new();
            let mut src = SliceSource::new(archive);
            let reader = ExoarReader::new(config);
            let _ = reader.read(&mut src, &mut receiver);
            // Construire le flux import depuis les blobs collectés
            let blobs = receiver.into_blobs();
            let mut k = 0usize;
            while k < blobs.len() {
                let (bid, data) = &blobs[k];
                builder.append_blob(*bid, data)?;
                k = k.wrapping_add(1);
            }
        }

        let mut src = SliceImportSource::new(builder.as_slice());
        let report = importer.run(&mut src, writer)?;

        self.summary
            .record_import(report.blobs_imported, report.bytes_written);
        if report.has_errors() {
            self.summary.record_error();
        }

        if self.config.audit_enabled {
            EXPORT_AUDIT.log_event(
                0,
                if report.is_clean() {
                    ExportEvent::SessionCompleted
                } else {
                    ExportEvent::SessionFailed
                },
            );
        }
        Ok(report)
    }

    /// Vérifie l'intégrité d'une archive ExoAR.
    pub fn verify_archive(&self, archive: &[u8]) -> ExofsResult<ExoarReadReport> {
        let config = ExoarReaderConfig::strict();
        let reader = ExoarReader::new(config);
        let mut receiver = CollectingReceiver::new();
        let mut src = SliceSource::new(archive);
        Ok(reader.read(&mut src, &mut receiver)?)
    }

    /// Retourne l'état de santé du module.
    pub fn health_check(&self) -> ExportHealthStatus {
        let audit_errors = EXPORT_AUDIT.count_errors_in_last_n(64);
        let summary_ok = self.summary.is_clean();
        match (audit_errors, summary_ok) {
            (0, true) => ExportHealthStatus::Healthy,
            (e, _) if e > 10 => ExportHealthStatus::Failed,
            _ => ExportHealthStatus::Degraded,
        }
    }

    /// Retourne le résumé des opérations.
    pub fn summary(&self) -> &ExportModuleSummary {
        &self.summary
    }

    /// Retourne la configuration actuelle.
    pub fn config(&self) -> &ExportConfig {
        &self.config
    }
}

// ─── Fonctions de commodité ───────────────────────────────────────────────────

/// Exporte un seul blob vers une archive ExoAR en mémoire.
pub fn export_blob_to_archive(blob_id: &[u8; 32], data: &[u8]) -> ExofsResult<Vec<u8>> {
    let opts = ExoarWriteOptions::default();
    let mut writer = ExoarBufferedWriter::new(opts);
    writer.begin()?;
    writer.write_blob(blob_id, data, 0, 0)?;
    let (buf, _) = writer
        .finalize()
        .map_err(|_| crate::fs::exofs::core::ExofsError::InternalError)?;
    Ok(buf)
}

/// Importe le premier blob d'une archive ExoAR en mémoire.
/// Retourne (blob_id, data) ou `ExofsError::NotFound` si aucune entrée.
pub fn import_first_from_archive(archive: &[u8]) -> ExofsResult<([u8; 32], Vec<u8>)> {
    let config = ExoarReaderConfig::default();
    let reader = ExoarReader::new(config);
    let mut receiver = CollectingReceiver::new();
    let mut src = SliceSource::new(archive);
    reader.read(&mut src, &mut receiver)?;
    let blobs = receiver.into_blobs();
    blobs.into_iter().next().ok_or(ExofsError::NotFound)
}

/// Vérifie qu'une archive ExoAR est valide (magic, CRC, entry count).
pub fn verify_archive(archive: &[u8]) -> ExofsResult<bool> {
    let config = ExoarReaderConfig::default();
    let reader = ExoarReader::new(config);
    let mut receiver = CollectingReceiver::new();
    let mut src = SliceSource::new(archive);
    match reader.read(&mut src, &mut receiver) {
        Ok(report) => Ok(!report.has_errors()),
        Err(_) => Ok(false),
    }
}

/// Convertit une archive ExoAR en flux tar.
pub fn exoar_to_tar(archive: &[u8], mtime: u64) -> ExofsResult<Vec<u8>> {
    let config = ExoarReaderConfig::default();
    let reader = ExoarReader::new(config);
    let mut receiver = CollectingReceiver::new();
    let mut src = SliceSource::new(archive);
    reader.read(&mut src, &mut receiver)?;
    let blobs = receiver.into_blobs();

    let mut sink = VecTarSink::new();
    let mut converter = ExoarToTarConverter::new();
    // Prépare les triplets (blob_id, name, data)
    let mut triplets: Vec<([u8; 32], Vec<u8>, Vec<u8>)> = Vec::new();
    triplets
        .try_reserve(blobs.len())
        .map_err(|_| ExofsError::NoMemory)?;

    // RECUR-01 : boucle while
    let mut i = 0usize;
    while i < blobs.len() {
        let (bid, data) = &blobs[i];
        let name = hex_name_from_id(bid);
        let mut d = Vec::new();
        d.try_reserve(data.len())
            .map_err(|_| ExofsError::NoMemory)?;
        d.extend_from_slice(data);
        triplets.push((*bid, name, d));
        i = i.wrapping_add(1);
    }

    // Construction des références
    let mut refs: Vec<([u8; 32], &[u8], &[u8])> = Vec::new();
    refs.try_reserve(triplets.len())
        .map_err(|_| ExofsError::NoMemory)?;
    let mut j = 0usize;
    while j < triplets.len() {
        refs.push((
            triplets[j].0,
            triplets[j].1.as_slice(),
            triplets[j].2.as_slice(),
        ));
        j = j.wrapping_add(1);
    }

    converter.convert(&mut sink, &refs, mtime)?;
    Ok(sink.as_slice().to_vec())
}

/// Génère un nom de fichier tar depuis un blob_id (16 premiers hex + ".exo").
fn hex_name_from_id(id: &[u8; 32]) -> Vec<u8> {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut name: Vec<u8> = Vec::new();
    let _ = name.try_reserve(20);
    let mut i = 0usize;
    // RECUR-01 : boucle while — 8 premiers bytes = 16 chars hex
    while i < 8 {
        let b = id[i];
        name.push(HEX[(b >> 4) as usize]);
        name.push(HEX[(b & 0xF) as usize]);
        i = i.wrapping_add(1);
    }
    name.extend_from_slice(b".exo");
    name
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_blob_id(n: u8) -> [u8; 32] {
        let mut id = [0u8; 32];
        id[0] = n;
        id[1] = 0xFF;
        id
    }

    #[test]
    fn test_export_config_default() {
        let cfg = ExportConfig::default_config(1);
        assert!(cfg.audit_enabled);
        assert!(cfg.verify_blob_ids);
        assert_eq!(cfg.session_id, 1);
    }

    #[test]
    fn test_export_config_fast() {
        let cfg = ExportConfig::fast(2);
        assert!(!cfg.audit_enabled);
        assert!(!cfg.verify_blob_ids);
    }

    #[test]
    fn test_export_config_import_config() {
        let cfg = ExportConfig::default_config(3);
        let ic = cfg.import_config();
        assert_eq!(ic.session_id, 3);
    }

    #[test]
    fn test_health_status_operational() {
        assert!(ExportHealthStatus::Healthy.is_operational());
        assert!(ExportHealthStatus::Degraded.is_operational());
        assert!(!ExportHealthStatus::Failed.is_operational());
    }

    #[test]
    fn test_module_summary_record() {
        let mut s = ExportModuleSummary::new();
        s.record_export(10, 1024);
        assert_eq!(s.total_exports, 1);
        assert_eq!(s.total_blobs_exported, 10);
        assert_eq!(s.total_bytes_exported, 1024);
        s.record_import(5, 512);
        assert_eq!(s.total_imports, 1);
        assert!(s.is_clean());
    }

    #[test]
    fn test_export_blob_to_archive_roundtrip() {
        let bid = dummy_blob_id(7);
        let data = b"roundtrip test data";
        let archive = export_blob_to_archive(&bid, data).expect("export ok");
        assert!(!archive.is_empty());

        let ok = verify_archive(&archive).expect("verify ok");
        assert!(ok);
    }

    #[test]
    fn test_import_first_from_archive() {
        let bid = dummy_blob_id(9);
        let data = b"import first test";
        let archive = export_blob_to_archive(&bid, data).expect("export ok");

        let (got_id, got_data) = import_first_from_archive(&archive).expect("import ok");
        assert_eq!(got_id, bid);
        assert_eq!(got_data, data);
    }

    #[test]
    fn test_verify_archive_invalid() {
        let bad = b"this is not an archive";
        let ok = verify_archive(bad).expect("no panic");
        assert!(!ok);
    }

    #[test]
    fn test_export_module_init() {
        let cfg = ExportConfig::default_config(1);
        let module = ExportModule::init(cfg);
        assert_eq!(module.summary().total_sessions, 0);
    }

    #[test]
    fn test_hex_name_from_id() {
        let id = [0xABu8; 32];
        let name = hex_name_from_id(&id);
        assert!(name.starts_with(b"abababababababab"));
        assert!(name.ends_with(b".exo"));
    }

    #[test]
    fn test_verify_archive_empty() {
        let ok = verify_archive(&[]).expect("no panic");
        assert!(!ok);
    }

    #[test]
    fn test_export_config_stream_export() {
        let cfg = ExportConfig::default_config(5);
        let sc = cfg.stream_export_config();
        assert_eq!(sc.session_id, 5);
        assert_eq!(sc.block_size, 65536);
    }
}
