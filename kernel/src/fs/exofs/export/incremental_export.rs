//! incremental_export.rs — Export incrémental basé sur les epochs ExoFS (no_std).
//!
//! Ce module fournit :
//!  - `IncrementalBlobSource` : trait d'accès aux blobs par plage d'epoch.
//!  - `IncrementalExport`     : moteur d'export incrémental (nouveaux + suppressions).
//!  - `IncrementalExportConfig` : configuration de l'export (filtres, limites).
//!  - `IncrementalExportResult` : rapport détaillé de l'export.
//!  - `DiffSet`               : ensemble de blobs ajoutés/supprimés entre deux epochs.
//!  - `EpochRange`            : intervalle d'epochs pour filtrage.
//!  - `SnapshotExport`        : export snapshot complet (epoch 0 → cible).
//!
//! RÈGLE 8  : magic écrit EN PREMIER — délégué à ExoarWriter.
//! RÈGLE 11 : BlobId = blake3(données brutes) — délégué à ExoarWriter.
//! RECUR-01 : pas de récursion — boucles for/while.
//! OOM-02   : try_reserve avant tout push.
//! ARITH-02 : saturating_* sur tous les compteurs.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::export_audit::{EXPORT_AUDIT, ExportEvent, ExportSession};
use super::exoar_writer::{ExoarWriter, ExoarWriteOptions, ArchiveSink};

// ─── Identifiant d'epoch ─────────────────────────────────────────────────────

/// Identifiant d'epoch (compteur monotone de génération ExoFS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EpochId(pub u64);

impl EpochId {
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(u64::MAX);

    #[inline] pub fn value(&self) -> u64 { self.0 }

    pub fn next(&self) -> Self {
        Self(self.0.saturating_add(1))
    }

    pub fn is_zero(&self) -> bool { self.0 == 0 }
}

// ─── Plage d'epochs ──────────────────────────────────────────────────────────

/// Intervalle [base, target] d'epochs pour filtrage des blobs.
#[derive(Clone, Copy, Debug)]
pub struct EpochRange {
    /// Epoch de départ (exclu dans l'incrémental).
    pub base: EpochId,
    /// Epoch cible (inclu).
    pub target: EpochId,
}

impl EpochRange {
    pub fn new(base: EpochId, target: EpochId) -> Self {
        Self { base, target }
    }

    /// Retourne true si `epoch` est dans la plage (base < epoch ≤ target).
    pub fn contains_exclusive(&self, epoch: EpochId) -> bool {
        epoch > self.base && epoch <= self.target
    }

    /// Retourne true si `epoch` est dans la plage (base ≤ epoch ≤ target).
    pub fn contains_inclusive(&self, epoch: EpochId) -> bool {
        epoch >= self.base && epoch <= self.target
    }

    /// Retourne le nombre d'epochs dans la plage.
    pub fn width(&self) -> u64 {
        self.target.0.saturating_sub(self.base.0)
    }

    /// Retourne true si la plage est un snapshot complet (base = 0).
    pub fn is_full_snapshot(&self) -> bool { self.base.is_zero() }
}

// ─── DiffSet ─────────────────────────────────────────────────────────────────

/// Ensemble de BlobIds modifiés entre deux epochs.
/// Séparé en blobs ajoutés et blobs supprimés.
pub struct DiffSet {
    /// BlobIds créés ou modifiés dans la plage.
    pub added: Vec<[u8; 32]>,
    /// BlobIds supprimés dans la plage (tombstones).
    pub deleted: Vec<[u8; 32]>,
    /// Range d'epochs correspondant à ce diff.
    pub range: EpochRange,
}

impl DiffSet {
    /// Crée un DiffSet vide.
    pub fn new(range: EpochRange) -> Self {
        Self { added: Vec::new(), deleted: Vec::new(), range }
    }

    /// Ajoute un BlobId dans la liste des ajouts — OOM-02 : try_reserve.
    pub fn push_added(&mut self, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.added.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.added.push(blob_id);
        Ok(())
    }

    /// Ajoute un BlobId dans la liste des suppressions — OOM-02 : try_reserve.
    pub fn push_deleted(&mut self, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.deleted.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.deleted.push(blob_id);
        Ok(())
    }

    pub fn total_blobs(&self) -> usize {
        self.added.len().saturating_add(self.deleted.len())
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.deleted.is_empty()
    }
}

// ─── Trait source de blobs ───────────────────────────────────────────────────

/// Trait d'accès aux blobs par plage d'epoch.
/// RECUR-01 : ne pas s'appeler récursivement dans les implémentations.
pub trait IncrementalBlobSource {
    /// Retourne le DiffSet entre `base` et `target`.
    /// OOM-02 : l'implémentation doit utiliser try_reserve.
    fn diff(&self, range: EpochRange) -> ExofsResult<DiffSet>;

    /// Lit les données brutes d'un blob donné son BlobId.
    /// Retourne Err(ObjectNotFound) si le blob n'existe pas.
    fn read_blob(&self, blob_id: &[u8; 32]) -> ExofsResult<Vec<u8>>;

    /// Retourne l'epoch de création d'un blob.
    fn blob_epoch(&self, blob_id: &[u8; 32]) -> ExofsResult<EpochId>;

    /// Retourne l'epoch courante du système.
    fn current_epoch(&self) -> EpochId;
}

// ─── Configuration de l'export incrémental ───────────────────────────────────

/// Configuration d'une session d'export incrémental.
#[derive(Clone, Copy, Debug)]
pub struct IncrementalExportConfig {
    /// Identifiant de session (unique par session active).
    pub session_id: u32,
    /// Epoch de base (état précédent déjà exporté).
    pub epoch_base: EpochId,
    /// Epoch cible (état à exporter).
    pub epoch_target: EpochId,
    /// Inclure les tombstones dans l'archive.
    pub include_tombstones: bool,
    /// Vérifier le BlobId = blake3(données) — RÈGLE 11.
    pub verify_blob_ids: bool,
    /// Nombre maximal de blobs à exporter (0 = illimité).
    pub max_blobs: u32,
    /// Taille maximale totale en bytes (0 = illimitée).
    pub max_bytes: u64,
    /// Annuler si un blob est introuvable (plutôt que continuer).
    pub fail_on_missing_blob: bool,
}

impl IncrementalExportConfig {
    pub const fn new(session_id: u32, epoch_base: EpochId, epoch_target: EpochId) -> Self {
        Self {
            session_id,
            epoch_base,
            epoch_target,
            include_tombstones: true,
            verify_blob_ids: false,
            max_blobs: 0,
            max_bytes: 0,
            fail_on_missing_blob: false,
        }
    }

    pub const fn snapshot(session_id: u32, epoch_target: EpochId) -> Self {
        Self::new(session_id, EpochId::ZERO, epoch_target)
    }

    pub fn validate(&self) -> ExofsResult<()> {
        if self.epoch_target < self.epoch_base {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }

    pub fn epoch_range(&self) -> EpochRange {
        EpochRange::new(self.epoch_base, self.epoch_target)
    }

    pub fn is_incremental(&self) -> bool {
        !self.epoch_base.is_zero()
    }
}

// ─── Résultat de l'export incrémental ────────────────────────────────────────

/// Rapport détaillé d'une session d'export incrémental.
#[derive(Clone, Copy, Debug, Default)]
pub struct IncrementalExportResult {
    /// Blobs ajoutés dans l'archive.
    pub blobs_exported: u32,
    /// Tombstones ajoutés dans l'archive.
    pub tombstones_exported: u32,
    /// Octets totaux de payload écrits.
    pub bytes_exported: u64,
    /// Blobs ignorés (filtrés ou introuvables).
    pub blobs_skipped: u32,
    /// Erreurs rencontrées.
    pub errors: u32,
    /// Epoch de base de cet export.
    pub epoch_base: u64,
    /// Epoch cible de cet export.
    pub epoch_target: u64,
    /// Taille totale de l'archive générée.
    pub archive_size: u64,
    /// true si l'export s'est terminé sans erreur.
    pub success: bool,
}

impl IncrementalExportResult {
    pub const fn new() -> Self {
        Self {
            blobs_exported: 0, tombstones_exported: 0,
            bytes_exported: 0, blobs_skipped: 0, errors: 0,
            epoch_base: 0, epoch_target: 0, archive_size: 0,
            success: false,
        }
    }

    pub fn total_entries(&self) -> u32 {
        self.blobs_exported.saturating_add(self.tombstones_exported)
    }

    pub fn is_empty(&self) -> bool { self.total_entries() == 0 }

    pub fn has_errors(&self) -> bool { self.errors > 0 }
}

// ─── Moteur d'export incrémental ─────────────────────────────────────────────

/// Moteur d'export incrémental ExoAR.
///
/// Séquence :
///   1. Calculer le DiffSet(epoch_base, epoch_target)
///   2. ExoarWriter::begin() — RÈGLE 8 : magic header EN PREMIER
///   3. Pour chaque blob ajouté : lire les données, calculate BlobId (RÈGLE 11), écrire l'entrée
///   4. Pour chaque blob supprimé : écrire un tombstone
///   5. ExoarWriter::finalize()
pub struct IncrementalExport {
    config: IncrementalExportConfig,
}

impl IncrementalExport {
    pub const fn new(config: IncrementalExportConfig) -> Self {
        Self { config }
    }

    /// Exécute l'export incrémental vers `sink`.
    /// RECUR-01 : deux boucles while (blobs ajoutés, blobs supprimés).
    pub fn run<S, Src>(
        &self,
        sink: &mut S,
        source: &Src,
    ) -> ExofsResult<IncrementalExportResult>
    where
        S: ArchiveSink,
        Src: IncrementalBlobSource,
    {
        self.config.validate()?;

        let mut result = IncrementalExportResult::new();
        result.epoch_base   = self.config.epoch_base.value();
        result.epoch_target = self.config.epoch_target.value();

        // Démarrer la session d'audit
        let mut session = ExportSession::new_export(self.config.session_id, result.epoch_base);
        session.start(&EXPORT_AUDIT);

        // Calculer le diff entre les deux epochs
        let range = self.config.epoch_range();
        let diff = match source.diff(range) {
            Ok(d) => d,
            Err(e) => {
                session.fail(&EXPORT_AUDIT);
                return Err(e);
            }
        };

        // Configurer l'écrivain ExoAR
        let write_opts = if self.config.is_incremental() {
            ExoarWriteOptions::incremental(result.epoch_base, result.epoch_target)
        } else {
            ExoarWriteOptions::snapshot(result.epoch_target)
        };
        let mut writer = ExoarWriter::new(write_opts);
        writer.begin(sink).map_err(|e| {
            session.fail(&EXPORT_AUDIT);
            ExofsError::from(e)
        })?;

        // ── Phase 1 : blobs ajoutés (RECUR-01 : boucle while) ──────────────
        let max_blobs = if self.config.max_blobs == 0 { u32::MAX } else { self.config.max_blobs };
        let max_bytes = if self.config.max_bytes == 0 { u64::MAX } else { self.config.max_bytes };

        let mut blob_idx = 0usize;
        while blob_idx < diff.added.len() {
            if result.blobs_exported >= max_blobs { break; }
            if result.bytes_exported >= max_bytes  { break; }

            let blob_id = &diff.added[blob_idx];
            let epoch = match source.blob_epoch(blob_id) {
                Ok(e) => e.value(),
                Err(_) => {
                    result.blobs_skipped = result.blobs_skipped.saturating_add(1);
                    blob_idx = blob_idx.wrapping_add(1);
                    continue;
                }
            };

            let data = match source.read_blob(blob_id) {
                Ok(d) => d,
                Err(ExofsError::ObjectNotFound) => {
                    if self.config.fail_on_missing_blob {
                        session.fail(&EXPORT_AUDIT);
                        return Err(ExofsError::ObjectNotFound);
                    }
                    result.blobs_skipped = result.blobs_skipped.saturating_add(1);
                    blob_idx = blob_idx.wrapping_add(1);
                    continue;
                }
                Err(e) => {
                    result.errors = result.errors.saturating_add(1);
                    session.record_error(ExportEvent::ExportFailed, 1, &EXPORT_AUDIT);
                    if self.config.fail_on_missing_blob {
                        session.fail(&EXPORT_AUDIT);
                        return Err(e);
                    }
                    blob_idx = blob_idx.wrapping_add(1);
                    continue;
                }
            };

            writer.write_blob(sink, blob_id, &data, 0, epoch)
                .map_err(|e| ExofsError::from(e))?;

            let sz = data.len() as u64;
            session.record_blob(blob_id, sz, &EXPORT_AUDIT);
            result.blobs_exported   = result.blobs_exported.saturating_add(1);
            result.bytes_exported   = result.bytes_exported.saturating_add(sz);
            blob_idx = blob_idx.wrapping_add(1);
        }

        // ── Phase 2 : tombstones (RECUR-01 : boucle while) ─────────────────
        if self.config.include_tombstones {
            let mut del_idx = 0usize;
            while del_idx < diff.deleted.len() {
                let blob_id = &diff.deleted[del_idx];
                let epoch = source.blob_epoch(blob_id).unwrap_or(EpochId::ZERO).value();
                writer.write_tombstone(sink, blob_id, epoch)
                    .map_err(|e| ExofsError::from(e))?;
                EXPORT_AUDIT.log_event(self.config.session_id, ExportEvent::TombstoneExported);
                result.tombstones_exported = result.tombstones_exported.saturating_add(1);
                del_idx = del_idx.wrapping_add(1);
            }
        }

        // Finaliser l'archive
        let stats = writer.finalize(sink).map_err(|e| ExofsError::from(e))?;
        result.archive_size = stats.archive_bytes;
        result.success = !result.has_errors();
        if result.success {
            session.complete(&EXPORT_AUDIT);
        } else {
            session.fail(&EXPORT_AUDIT);
        }
        Ok(result)
    }
}

// ─── Export snapshot complet ─────────────────────────────────────────────────

/// Export snapshot complet (epoch 0 → epoch_target).
/// Identique à IncrementalExport avec epoch_base = 0.
pub struct SnapshotExport {
    export: IncrementalExport,
}

impl SnapshotExport {
    pub fn new(session_id: u32, epoch_target: EpochId) -> Self {
        let cfg = IncrementalExportConfig::snapshot(session_id, epoch_target);
        Self { export: IncrementalExport::new(cfg) }
    }

    pub fn run<S, Src>(&self, sink: &mut S, source: &Src) -> ExofsResult<IncrementalExportResult>
    where
        S: ArchiveSink,
        Src: IncrementalBlobSource,
    {
        self.export.run(sink, source)
    }
}

// ─── Export multi-epoch ───────────────────────────────────────────────────────

/// Résumé d'un export multi-epoch (plusieurs plages concaténées).
#[derive(Clone, Copy, Debug, Default)]
pub struct MultiEpochExportSummary {
    pub archives_created: u32,
    pub total_blobs: u64,
    pub total_bytes: u64,
    pub total_errors: u32,
}

impl MultiEpochExportSummary {
    pub const fn new() -> Self {
        Self { archives_created: 0, total_blobs: 0, total_bytes: 0, total_errors: 0 }
    }

    pub fn merge(&mut self, result: &IncrementalExportResult) {
        self.archives_created = self.archives_created.saturating_add(1);
        self.total_blobs = self.total_blobs.saturating_add(result.blobs_exported as u64);
        self.total_bytes = self.total_bytes.saturating_add(result.bytes_exported);
        self.total_errors = self.total_errors.saturating_add(result.errors);
    }

    pub fn has_errors(&self) -> bool { self.total_errors > 0 }
}

// ─── Source de test (mock) ────────────────────────────────────────────────────

/// Source de blobs factice pour les tests unitaires.
pub struct MockBlobSource {
    blobs: Vec<([u8; 32], Vec<u8>, EpochId)>,
    deleted: Vec<([u8; 32], EpochId)>,
    current_epoch: EpochId,
}

impl MockBlobSource {
    pub fn new(current_epoch: EpochId) -> Self {
        Self { blobs: Vec::new(), deleted: Vec::new(), current_epoch }
    }

    pub fn add_blob(&mut self, id: [u8; 32], data: &[u8], epoch: EpochId) {
        let mut v = Vec::new();
        let _ = v.try_reserve(data.len());
        v.extend_from_slice(data);
        let _ = self.blobs.try_reserve(1);
        self.blobs.push((id, v, epoch));
    }

    pub fn add_deleted(&mut self, id: [u8; 32], epoch: EpochId) {
        let _ = self.deleted.try_reserve(1);
        self.deleted.push((id, epoch));
    }
}

impl IncrementalBlobSource for MockBlobSource {
    fn diff(&self, range: EpochRange) -> ExofsResult<DiffSet> {
        let mut ds = DiffSet::new(range);
        for (id, _, epoch) in &self.blobs {
            if range.contains_exclusive(*epoch) {
                ds.push_added(*id)?;
            }
        }
        for (id, epoch) in &self.deleted {
            if range.contains_exclusive(*epoch) {
                ds.push_deleted(*id)?;
            }
        }
        Ok(ds)
    }

    fn read_blob(&self, blob_id: &[u8; 32]) -> ExofsResult<Vec<u8>> {
        for (id, data, _) in &self.blobs {
            if id == blob_id {
                let mut v = Vec::new();
                v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
                v.extend_from_slice(data);
                return Ok(v);
            }
        }
        Err(ExofsError::ObjectNotFound)
    }

    fn blob_epoch(&self, blob_id: &[u8; 32]) -> ExofsResult<EpochId> {
        for (id, _, epoch) in &self.blobs {
            if id == blob_id { return Ok(*epoch); }
        }
        for (id, epoch) in &self.deleted {
            if id == blob_id { return Ok(*epoch); }
        }
        Err(ExofsError::ObjectNotFound)
    }

    fn current_epoch(&self) -> EpochId { self.current_epoch }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::exoar_writer::SinkVec;
    use super::super::exoar_reader::{ExoarReader, ExoarReaderConfig, SliceSource, CollectingReceiver};

    fn make_id(tag: u8) -> [u8; 32] {
        let mut id = [0u8; 32];
        id[0] = tag;
        id
    }

    #[test]
    fn test_epoch_range_contains() {
        let r = EpochRange::new(EpochId(5), EpochId(10));
        assert!(r.contains_exclusive(EpochId(6)));
        assert!(!r.contains_exclusive(EpochId(5))); // exclu
        assert!(r.contains_exclusive(EpochId(10)));
        assert!(!r.contains_exclusive(EpochId(11)));
    }

    #[test]
    fn test_diff_set_push() {
        let r = EpochRange::new(EpochId(0), EpochId(10));
        let mut ds = DiffSet::new(r);
        ds.push_added(make_id(1)).expect("push ok");
        ds.push_deleted(make_id(2)).expect("push ok");
        assert_eq!(ds.added.len(), 1);
        assert_eq!(ds.deleted.len(), 1);
        assert_eq!(ds.total_blobs(), 2);
    }

    #[test]
    fn test_incremental_export_empty() {
        let src = MockBlobSource::new(EpochId(10));
        let cfg = IncrementalExportConfig::new(1, EpochId(0), EpochId(10));
        let exporter = IncrementalExport::new(cfg);
        let mut sink = SinkVec::new();
        let result = exporter.run(&mut sink, &src).expect("export ok");
        assert_eq!(result.blobs_exported, 0);
        assert_eq!(result.tombstones_exported, 0);
        assert!(result.success);
    }

    #[test]
    fn test_incremental_export_single_blob() {
        let mut src = MockBlobSource::new(EpochId(5));
        src.add_blob(make_id(7), b"test data", EpochId(3));
        let cfg = IncrementalExportConfig::new(1, EpochId(0), EpochId(5));
        let exporter = IncrementalExport::new(cfg);
        let mut sink = SinkVec::new();
        let result = exporter.run(&mut sink, &src).expect("export ok");
        assert_eq!(result.blobs_exported, 1);
        assert!(result.bytes_exported > 0);
    }

    #[test]
    fn test_incremental_export_with_tombstone() {
        let mut src = MockBlobSource::new(EpochId(10));
        src.add_deleted(make_id(5), EpochId(8));
        let cfg = IncrementalExportConfig::new(2, EpochId(5), EpochId(10));
        let exporter = IncrementalExport::new(cfg);
        let mut sink = SinkVec::new();
        let result = exporter.run(&mut sink, &src).expect("export");
        assert_eq!(result.tombstones_exported, 1);
    }

    #[test]
    fn test_incremental_only_new_blobs() {
        let mut src = MockBlobSource::new(EpochId(10));
        src.add_blob(make_id(1), b"old", EpochId(2));  // avant base
        src.add_blob(make_id(2), b"new", EpochId(7));  // dans plage
        let cfg = IncrementalExportConfig::new(1, EpochId(5), EpochId(10));
        let exporter = IncrementalExport::new(cfg);
        let mut sink = SinkVec::new();
        let result = exporter.run(&mut sink, &src).expect("export");
        assert_eq!(result.blobs_exported, 1); // seulement le blob epoch 7
    }

    #[test]
    fn test_snapshot_export() {
        let mut src = MockBlobSource::new(EpochId(5));
        src.add_blob(make_id(1), b"snap1", EpochId(1));
        src.add_blob(make_id(2), b"snap2", EpochId(3));
        let exporter = SnapshotExport::new(10, EpochId(5));
        let mut sink = SinkVec::new();
        let result = exporter.run(&mut sink, &src).expect("snapshot");
        assert_eq!(result.blobs_exported, 2);
        assert!(result.success);
    }

    #[test]
    fn test_export_roundtrip_via_reader() {
        let mut src = MockBlobSource::new(EpochId(3));
        let id1 = make_id(10);
        src.add_blob(id1, b"content one", EpochId(1));
        let id2 = make_id(20);
        src.add_blob(id2, b"content two larger", EpochId(2));
        let cfg = IncrementalExportConfig::new(5, EpochId(0), EpochId(3));
        let exporter = IncrementalExport::new(cfg);
        let mut sink = SinkVec::new();
        exporter.run(&mut sink, &src).expect("export");
        let data = sink.into_inner();
        let mut slsrc = SliceSource::new(&data);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let report = reader.read(&mut slsrc, &mut rcv).expect("read");
        assert_eq!(report.entries_read, 2);
    }

    #[test]
    fn test_max_blobs_limit() {
        let mut src = MockBlobSource::new(EpochId(5));
        for i in 0..10u8 { src.add_blob(make_id(i), b"data", EpochId(1)); }
        let mut cfg = IncrementalExportConfig::new(1, EpochId(0), EpochId(5));
        cfg.max_blobs = 3;
        let exporter = IncrementalExport::new(cfg);
        let mut sink = SinkVec::new();
        let result = exporter.run(&mut sink, &src).expect("export");
        assert_eq!(result.blobs_exported, 3);
    }

    #[test]
    fn test_multi_epoch_summary_merge() {
        let mut summary = MultiEpochExportSummary::new();
        let mut r = IncrementalExportResult::new();
        r.blobs_exported = 5;
        r.bytes_exported = 1024;
        summary.merge(&r);
        summary.merge(&r);
        assert_eq!(summary.archives_created, 2);
        assert_eq!(summary.total_blobs, 10);
        assert_eq!(summary.total_bytes, 2048);
    }

    #[test]
    fn test_epoch_range_width() {
        let r = EpochRange::new(EpochId(3), EpochId(7));
        assert_eq!(r.width(), 4);
    }

    #[test]
    fn test_epoch_id_next() {
        let e = EpochId(5);
        assert_eq!(e.next(), EpochId(6));
        assert_eq!(EpochId::MAX.next(), EpochId::MAX); // saturating
    }

    #[test]
    fn test_config_validation_bad_range() {
        let cfg = IncrementalExportConfig::new(1, EpochId(10), EpochId(5));
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_config_is_incremental() {
        let cfg_inc = IncrementalExportConfig::new(1, EpochId(3), EpochId(5));
        assert!(cfg_inc.is_incremental());
        let cfg_snap = IncrementalExportConfig::snapshot(1, EpochId(5));
        assert!(!cfg_snap.is_incremental());
    }
}
