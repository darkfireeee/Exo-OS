//! export_audit.rs — Journal d'audit temps-réel des opérations export/import
//! (no_std, Ring0, thread-safe par AtomicU64 + UnsafeCell).
//!
//! Ce module fournit :
//!  - `ExportAuditLog`    : anneau circulaire de 512 entrées d'audit.
//!  - `ExportEvent`       : énumération de tous les événements traçables.
//!  - `ExportAuditEntry`  : structure de 64 bytes par entrée d'audit.
//!  - `ExportAuditStats`  : compteurs agrégés par type d'événement.
//!  - `ExportAuditQuery`  : consultation des N dernières entrées.
//!  - `ExportSession`     : session d'export/import avec suivi d'état.
//!  - `ExportSessionConfig`: paramètres d'une session.
//!
//! RECUR-01 : pas de récursion — boucles while/for.
//! ARITH-02 : saturating_* / wrapping_* sur les compteurs.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── Constantes ───────────────────────────────────────────────────────────────
/// Capacité de l'anneau circulaire (puissance de 2 pour masquage).
pub const EXPORT_AUDIT_RING: usize = 512;
const RING_MASK: usize = EXPORT_AUDIT_RING - 1;

// ─── Singleton global ────────────────────────────────────────────────────────
/// Journal d'audit global du module export.
pub static EXPORT_AUDIT: ExportAuditLog = ExportAuditLog::new_const();

// ─── Événements d'audit ───────────────────────────────────────────────────────

/// Événements traçables dans le journal d'audit export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExportEvent {
    /// Session d'export démarrée.
    ExportStarted = 0,
    /// Session d'export terminée avec succès.
    ExportCompleted = 1,
    /// Session d'export interrompue par erreur.
    ExportFailed = 2,
    /// Session d'import démarrée.
    ImportStarted = 3,
    /// Session d'import terminée avec succès.
    ImportCompleted = 4,
    /// Session d'import interrompue par erreur.
    ImportFailed = 5,
    /// Blob exporté vers l'archive.
    BlobExported = 6,
    /// Blob importé depuis l'archive.
    BlobImported = 7,
    /// Tombstone exporté.
    TombstoneExported = 8,
    /// Tombstone importé.
    TombstoneImported = 9,
    /// Erreur CRC32C détectée sur un payload.
    CrcError = 10,
    /// BlobId ne correspond pas au hash des données (RÈGLE 11).
    BlobIdMismatch = 11,
    /// Magic invalide détecté dans l'archive (RÈGLE 8).
    MagicError = 12,
    /// Conflit de blob résolu (blob déjà présent).
    ConflictResolved = 13,
    /// Entrée ignorée (filtrée par politique).
    EntrySkipped = 14,
    /// Archive compressée.
    CompressionApplied = 15,
    /// Archive décompressée.
    DecompressionDone = 16,
    /// Vérification d'intégrité réussie.
    IntegrityVerified = 17,
    /// Vérification d'intégrité échouée.
    IntegrityFailed = 18,
    /// Checkpoint de reprise enregistré.
    CheckpointSaved = 19,
    /// Session d'export/import démarrée (alias générique).
    SessionStarted = 20,
    /// Session d'export/import terminée avec succès (alias générique).
    SessionCompleted = 21,
    /// Session d'export/import interrompue par erreur (alias générique).
    SessionFailed = 22,
    /// Export incrémental effectué.
    IncrementalExport = 23,
    /// Événement inconnu / réservé.
    Unknown = 255,
}

impl ExportEvent {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::ExportStarted,
            1 => Self::ExportCompleted,
            2 => Self::ExportFailed,
            3 => Self::ImportStarted,
            4 => Self::ImportCompleted,
            5 => Self::ImportFailed,
            6 => Self::BlobExported,
            7 => Self::BlobImported,
            8 => Self::TombstoneExported,
            9 => Self::TombstoneImported,
            10 => Self::CrcError,
            11 => Self::BlobIdMismatch,
            12 => Self::MagicError,
            13 => Self::ConflictResolved,
            14 => Self::EntrySkipped,
            15 => Self::CompressionApplied,
            16 => Self::DecompressionDone,
            17 => Self::IntegrityVerified,
            18 => Self::IntegrityFailed,
            19 => Self::CheckpointSaved,
            _ => Self::Unknown,
        }
    }

    /// Retourne true si l'événement représente une erreur.
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            Self::ExportFailed
                | Self::ImportFailed
                | Self::CrcError
                | Self::BlobIdMismatch
                | Self::MagicError
                | Self::IntegrityFailed
        )
    }

    /// Retourne true si l'événement concerne un blob individuel.
    pub fn is_blob_event(&self) -> bool {
        matches!(
            self,
            Self::BlobExported | Self::BlobImported | Self::BlobIdMismatch
        )
    }
}

// ─── Entrée d'audit ───────────────────────────────────────────────────────────

/// Entrée d'audit : 64 bytes, alignée et copiable.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ExportAuditEntry {
    /// Type d'événement.
    pub event: ExportEvent,
    /// Padding après enum.
    pub _pad: [u8; 3],
    /// Identifiant de session (session_id).
    pub session_id: u32,
    /// Séquence monotone (numéro d'ordre global).
    pub seq: u64,
    /// Epoch ou timestamp (selon contexte).
    pub epoch: u64,
    /// BlobId associé (8 premiers bytes pour compacité).
    pub blob_id_prefix: [u8; 8],
    /// Bytes supplémentaires de BlobId.
    pub blob_id_mid: [u8; 16],
    /// Taille du payload impliqué (0 si non applicable).
    pub payload_size: u64,
    /// Code d'erreur (0 = succès).
    pub error_code: u32,
    /// Padding final pour atteindre 64 bytes.
    pub _pad2: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<ExportAuditEntry>() == 64);

impl ExportAuditEntry {
    /// Crée une entrée d'audit vide.
    pub const fn new_empty() -> Self {
        Self {
            event: ExportEvent::Unknown,
            _pad: [0u8; 3],
            session_id: 0,
            seq: 0,
            epoch: 0,
            blob_id_prefix: [0u8; 8],
            blob_id_mid: [0u8; 16],
            payload_size: 0,
            error_code: 0,
            _pad2: [0u8; 4],
        }
    }

    /// Crée une entrée de blob exporté.
    pub fn blob_exported(session_id: u32, blob_id: &[u8; 32], size: u64, epoch: u64) -> Self {
        let mut e = Self::new_empty();
        e.event = ExportEvent::BlobExported;
        e.session_id = session_id;
        e.epoch = epoch;
        e.payload_size = size;
        e.blob_id_prefix.copy_from_slice(&blob_id[..8]);
        e.blob_id_mid.copy_from_slice(&blob_id[8..24]);
        e
    }

    /// Crée une entrée de blob importé.
    pub fn blob_imported(session_id: u32, blob_id: &[u8; 32], size: u64) -> Self {
        let mut e = Self::new_empty();
        e.event = ExportEvent::BlobImported;
        e.session_id = session_id;
        e.payload_size = size;
        e.blob_id_prefix.copy_from_slice(&blob_id[..8]);
        e.blob_id_mid.copy_from_slice(&blob_id[8..24]);
        e
    }

    /// Crée une entrée d'erreur.
    pub fn error(session_id: u32, event: ExportEvent, code: u32) -> Self {
        let mut e = Self::new_empty();
        e.event = event;
        e.session_id = session_id;
        e.error_code = code;
        e
    }

    /// Retourne true si l'entrée représente une erreur.
    pub fn is_error(&self) -> bool {
        self.event.is_error()
    }
}

// ─── Statistiques d'audit ────────────────────────────────────────────────────

/// Compteurs agrégés des événements d'audit.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExportAuditStats {
    pub exports_started: u32,
    pub exports_completed: u32,
    pub exports_failed: u32,
    pub imports_started: u32,
    pub imports_completed: u32,
    pub imports_failed: u32,
    pub blobs_exported: u64,
    pub blobs_imported: u64,
    pub tombstones_exported: u32,
    pub crc_errors: u32,
    pub magic_errors: u32,
    pub blob_id_mismatches: u32,
    pub total_events: u64,
}

impl ExportAuditStats {
    pub const fn new() -> Self {
        Self {
            exports_started: 0,
            exports_completed: 0,
            exports_failed: 0,
            imports_started: 0,
            imports_completed: 0,
            imports_failed: 0,
            blobs_exported: 0,
            blobs_imported: 0,
            tombstones_exported: 0,
            crc_errors: 0,
            magic_errors: 0,
            blob_id_mismatches: 0,
            total_events: 0,
        }
    }

    fn record(&mut self, event: ExportEvent) {
        self.total_events = self.total_events.saturating_add(1);
        match event {
            ExportEvent::ExportStarted => {
                self.exports_started = self.exports_started.saturating_add(1)
            }
            ExportEvent::ExportCompleted => {
                self.exports_completed = self.exports_completed.saturating_add(1)
            }
            ExportEvent::ExportFailed => {
                self.exports_failed = self.exports_failed.saturating_add(1)
            }
            ExportEvent::ImportStarted => {
                self.imports_started = self.imports_started.saturating_add(1)
            }
            ExportEvent::ImportCompleted => {
                self.imports_completed = self.imports_completed.saturating_add(1)
            }
            ExportEvent::ImportFailed => {
                self.imports_failed = self.imports_failed.saturating_add(1)
            }
            ExportEvent::BlobExported => {
                self.blobs_exported = self.blobs_exported.saturating_add(1)
            }
            ExportEvent::BlobImported => {
                self.blobs_imported = self.blobs_imported.saturating_add(1)
            }
            ExportEvent::TombstoneExported => {
                self.tombstones_exported = self.tombstones_exported.saturating_add(1)
            }
            ExportEvent::CrcError => self.crc_errors = self.crc_errors.saturating_add(1),
            ExportEvent::MagicError => self.magic_errors = self.magic_errors.saturating_add(1),
            ExportEvent::BlobIdMismatch => {
                self.blob_id_mismatches = self.blob_id_mismatches.saturating_add(1)
            }
            _ => {}
        }
    }

    /// Retourne true si aucune erreur n'a été enregistrée.
    pub fn is_clean(&self) -> bool {
        self.exports_failed == 0
            && self.imports_failed == 0
            && self.crc_errors == 0
            && self.magic_errors == 0
            && self.blob_id_mismatches == 0
    }
}

// ─── Journal d'audit (anneau circulaire, thead-safe) ─────────────────────────

/// Journal d'audit pour le module export — anneau circulaire, thread-safe.
/// Implémenté avec UnsafeCell + AtomicU64 spinlock (pattern Ring0).
pub struct ExportAuditLog {
    entries: UnsafeCell<[ExportAuditEntry; EXPORT_AUDIT_RING]>,
    /// Pointeur d'écriture (head), wrapping sur EXPORT_AUDIT_RING.
    head: AtomicU64,
    /// Compteur de séquence global.
    seq: AtomicU64,
    /// Spinlock (0 = libre, 1 = pris).
    lock: AtomicU64,
    /// Statistiques intégrées.
    stats: UnsafeCell<ExportAuditStats>,
}

unsafe impl Sync for ExportAuditLog {}
unsafe impl Send for ExportAuditLog {}

impl ExportAuditLog {
    /// Constructeur const pour le singleton statique.
    pub const fn new_const() -> Self {
        Self {
            entries: UnsafeCell::new([ExportAuditEntry::new_empty(); EXPORT_AUDIT_RING]),
            head: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            lock: AtomicU64::new(0),
            stats: UnsafeCell::new(ExportAuditStats::new()),
        }
    }

    /// Acquiert le spinlock (busy-wait, Ring0 safe).
    fn acquire(&self) {
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    /// Libère le spinlock.
    fn release(&self) {
        self.lock.store(0, Ordering::Release);
    }

    /// Enregistre un événement dans le journal.
    pub fn record(&self, mut entry: ExportAuditEntry) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        entry.seq = seq;
        self.acquire();
        let head = self.head.load(Ordering::Relaxed);
        let idx = (head as usize) & RING_MASK;
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        unsafe {
            let entries = &mut *self.entries.get();
            entries[idx] = entry;
            let stats = &mut *self.stats.get();
            stats.record(entry.event);
        }
        self.head.store(head.wrapping_add(1), Ordering::Relaxed);
        self.release();
    }

    /// Enregistre un événement simple (sans BlobId).
    pub fn log_event(&self, session_id: u32, event: ExportEvent) {
        let mut entry = ExportAuditEntry::new_empty();
        entry.event = event;
        entry.session_id = session_id;
        self.record(entry);
    }

    /// Enregistre un blob exporté.
    pub fn log_blob_exported(&self, session_id: u32, blob_id: &[u8; 32], size: u64, epoch: u64) {
        let entry = ExportAuditEntry::blob_exported(session_id, blob_id, size, epoch);
        self.record(entry);
    }

    /// Enregistre un blob importé.
    pub fn log_blob_imported(&self, session_id: u32, blob_id: &[u8; 32], size: u64) {
        let entry = ExportAuditEntry::blob_imported(session_id, blob_id, size);
        self.record(entry);
    }

    /// Enregistre une erreur.
    pub fn log_error(&self, session_id: u32, event: ExportEvent, code: u32) {
        let entry = ExportAuditEntry::error(session_id, event, code);
        self.record(entry);
    }

    /// Retourne une copie des statistiques actuelles.
    pub fn stats(&self) -> ExportAuditStats {
        self.acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let s = unsafe { *self.stats.get() };
        self.release();
        s
    }

    /// Retourne le nombre total d'événements enregistrés.
    pub fn total_events(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Copie les N dernières entrées dans `out` (RECUR-01 : boucle while).
    /// Retourne le nombre d'entrées copiées.
    pub fn last_n(&self, out: &mut [ExportAuditEntry]) -> usize {
        if out.is_empty() {
            return 0;
        }
        self.acquire();
        let head = self.head.load(Ordering::Relaxed) as usize;
        let total = self.seq.load(Ordering::Relaxed) as usize;
        let available = total.min(EXPORT_AUDIT_RING);
        let n = out.len().min(available);
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let entries_ref = unsafe { &*self.entries.get() };
        let mut i = 0usize;
        while i < n {
            let ring_pos = (head.wrapping_sub(n).wrapping_add(i)) & RING_MASK;
            out[i] = entries_ref[ring_pos];
            i = i.wrapping_add(1);
        }
        self.release();
        n
    }

    /// Retourne la dernière entrée enregistrée, ou None si le journal est vide.
    pub fn last_entry(&self) -> Option<ExportAuditEntry> {
        if self.seq.load(Ordering::Relaxed) == 0 {
            return None;
        }
        self.acquire();
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx = (head.wrapping_sub(1)) & RING_MASK;
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let entry = unsafe { (*self.entries.get())[idx] };
        self.release();
        Some(entry)
    }

    /// Retourne le nombre d'erreurs parmi les dernières `n` entrées.
    /// RECUR-01 : boucle while.
    pub fn count_errors_in_last_n(&self, n: usize) -> usize {
        let cap = n.min(EXPORT_AUDIT_RING);
        let mut buf = [ExportAuditEntry::new_empty(); EXPORT_AUDIT_RING];
        let got = self.last_n(&mut buf[..cap]);
        let mut count = 0usize;
        let mut i = 0usize;
        while i < got {
            if buf[i].is_error() {
                count = count.saturating_add(1);
            }
            i = i.wrapping_add(1);
        }
        count
    }

    /// Remet à zéro le journal et les statistiques.
    pub fn reset(&self) {
        self.acquire();
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        unsafe {
            let entries = &mut *self.entries.get();
            let mut i = 0usize;
            while i < EXPORT_AUDIT_RING {
                entries[i] = ExportAuditEntry::new_empty();
                i = i.wrapping_add(1);
            }
            *self.stats.get() = ExportAuditStats::new();
        }
        self.head.store(0, Ordering::Relaxed);
        self.seq.store(0, Ordering::Relaxed);
        self.release();
    }

    /// Retourne true si aucune erreur n'est dans les statistiques courantes.
    pub fn is_clean(&self) -> bool {
        self.stats().is_clean()
    }
}

// ─── Session d'export/import ─────────────────────────────────────────────────

/// État d'une session d'export ou d'import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Running,
    Completed,
    Failed,
}

/// Session d'export/import avec suivi de durée et d'état.
pub struct ExportSession {
    pub session_id: u32,
    pub state: SessionState,
    pub blobs_processed: u32,
    pub bytes_processed: u64,
    pub errors: u32,
    pub is_import: bool,
    pub epoch_start: u64,
    pub epoch_end: u64,
}

impl ExportSession {
    /// Crée une nouvelle session d'export.
    pub fn new_export(session_id: u32, epoch_start: u64) -> Self {
        Self {
            session_id,
            state: SessionState::Idle,
            blobs_processed: 0,
            bytes_processed: 0,
            errors: 0,
            is_import: false,
            epoch_start,
            epoch_end: 0,
        }
    }

    /// Crée une nouvelle session d'import.
    pub fn new_import(session_id: u32) -> Self {
        Self {
            session_id,
            state: SessionState::Idle,
            blobs_processed: 0,
            bytes_processed: 0,
            errors: 0,
            is_import: true,
            epoch_start: 0,
            epoch_end: 0,
        }
    }

    /// Démarre la session.
    pub fn start(&mut self, audit: &ExportAuditLog) {
        self.state = SessionState::Running;
        let event = if self.is_import {
            ExportEvent::ImportStarted
        } else {
            ExportEvent::ExportStarted
        };
        audit.log_event(self.session_id, event);
    }

    /// Enregistre un blob traité.
    pub fn record_blob(&mut self, blob_id: &[u8; 32], size: u64, audit: &ExportAuditLog) {
        self.blobs_processed = self.blobs_processed.saturating_add(1);
        self.bytes_processed = self.bytes_processed.saturating_add(size);
        if self.is_import {
            audit.log_blob_imported(self.session_id, blob_id, size);
        } else {
            audit.log_blob_exported(self.session_id, blob_id, size, self.epoch_start);
        }
    }

    /// Enregistre une erreur.
    pub fn record_error(&mut self, event: ExportEvent, code: u32, audit: &ExportAuditLog) {
        self.errors = self.errors.saturating_add(1);
        audit.log_error(self.session_id, event, code);
    }

    /// Termine la session avec succès.
    pub fn complete(&mut self, audit: &ExportAuditLog) {
        self.state = SessionState::Completed;
        let event = if self.is_import {
            ExportEvent::ImportCompleted
        } else {
            ExportEvent::ExportCompleted
        };
        audit.log_event(self.session_id, event);
    }

    /// Termine la session avec échec.
    pub fn fail(&mut self, audit: &ExportAuditLog) {
        self.state = SessionState::Failed;
        let event = if self.is_import {
            ExportEvent::ImportFailed
        } else {
            ExportEvent::ExportFailed
        };
        audit.log_event(self.session_id, event);
    }

    /// Retourne true si la session est terminée (succès ou échec).
    pub fn is_done(&self) -> bool {
        matches!(self.state, SessionState::Completed | SessionState::Failed)
    }

    /// Retourne true si aucune erreur n'a été enregistrée.
    pub fn is_clean(&self) -> bool {
        self.errors == 0
    }
}

// ─── Configuration de session ────────────────────────────────────────────────

/// Paramètre de configuration d'une session.
#[derive(Clone, Copy, Debug)]
pub struct ExportSessionConfig {
    /// Identifiant de session (doit être unique par session active).
    pub session_id: u32,
    /// Epoch de départ (export incrémental).
    pub epoch_base: u64,
    /// Epoch cible.
    pub epoch_target: u64,
    /// Vérifier le BlobId =  blake3(données) — RÈGLE 11.
    pub verify_blob_ids: bool,
    /// Vérifier les CRC32C des payloads.
    pub verify_crc: bool,
    /// Nombre maximal de blobs à traiter (0 = illimité).
    pub max_blobs: u32,
    /// Taille maximale totale en bytes (0 = illimitée).
    pub max_bytes: u64,
}

impl ExportSessionConfig {
    pub const fn default(session_id: u32) -> Self {
        Self {
            session_id,
            epoch_base: 0,
            epoch_target: u64::MAX,
            verify_blob_ids: false,
            verify_crc: true,
            max_blobs: 0,
            max_bytes: 0,
        }
    }

    pub const fn strict(session_id: u32) -> Self {
        Self {
            session_id,
            epoch_base: 0,
            epoch_target: u64::MAX,
            verify_blob_ids: true,
            verify_crc: true,
            max_blobs: 0,
            max_bytes: 0,
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_log() -> ExportAuditLog {
        ExportAuditLog::new_const()
    }

    #[test]
    fn test_log_single_event() {
        let log = fresh_log();
        log.log_event(1, ExportEvent::ExportStarted);
        assert_eq!(log.total_events(), 1);
    }

    #[test]
    fn test_stats_record_errors() {
        let log = fresh_log();
        log.log_error(1, ExportEvent::CrcError, 42);
        log.log_error(1, ExportEvent::MagicError, 1);
        let s = log.stats();
        assert_eq!(s.crc_errors, 1);
        assert_eq!(s.magic_errors, 1);
        assert!(!s.is_clean());
    }

    #[test]
    fn test_stats_blob_events() {
        let log = fresh_log();
        let bid = [5u8; 32];
        log.log_blob_exported(1, &bid, 1024, 10);
        log.log_blob_imported(2, &bid, 512);
        let s = log.stats();
        assert_eq!(s.blobs_exported, 1);
        assert_eq!(s.blobs_imported, 1);
    }

    #[test]
    fn test_last_entry() {
        let log = fresh_log();
        assert!(log.last_entry().is_none());
        log.log_event(1, ExportEvent::ExportStarted);
        log.log_event(1, ExportEvent::ExportCompleted);
        let e = log.last_entry().expect("has entry");
        assert_eq!(e.event, ExportEvent::ExportCompleted);
    }

    #[test]
    fn test_last_n() {
        let log = fresh_log();
        for i in 0..10u32 {
            log.log_event(i, ExportEvent::BlobExported);
        }
        let mut buf = [ExportAuditEntry::new_empty(); 5];
        let n = log.last_n(&mut buf);
        assert_eq!(n, 5);
    }

    #[test]
    fn test_ring_wraps_around() {
        let log = fresh_log();
        for i in 0..600u32 {
            log.log_event(i, ExportEvent::BlobExported);
        }
        assert_eq!(log.total_events(), 600);
        let e = log.last_entry().expect("has entry");
        assert_eq!(e.event, ExportEvent::BlobExported);
    }

    #[test]
    fn test_reset() {
        let log = fresh_log();
        log.log_event(1, ExportEvent::ExportStarted);
        log.reset();
        assert_eq!(log.total_events(), 0);
        assert!(log.last_entry().is_none());
        assert!(log.stats().is_clean());
    }

    #[test]
    fn test_count_errors_in_last_n() {
        let log = fresh_log();
        log.log_event(1, ExportEvent::BlobExported);
        log.log_error(1, ExportEvent::CrcError, 1);
        log.log_error(1, ExportEvent::MagicError, 2);
        let errs = log.count_errors_in_last_n(10);
        assert_eq!(errs, 2);
    }

    #[test]
    fn test_session_export_flow() {
        let log = fresh_log();
        let mut sess = ExportSession::new_export(42, 100);
        sess.start(&log);
        let bid = [7u8; 32];
        sess.record_blob(&bid, 2048, &log);
        sess.complete(&log);
        assert_eq!(sess.state, SessionState::Completed);
        assert_eq!(sess.blobs_processed, 1);
        assert!(sess.is_clean());
    }

    #[test]
    fn test_session_import_fail() {
        let log = fresh_log();
        let mut sess = ExportSession::new_import(99);
        sess.start(&log);
        sess.record_error(ExportEvent::CrcError, 5, &log);
        sess.fail(&log);
        assert!(sess.is_done());
        assert_eq!(sess.errors, 1);
    }

    #[test]
    fn test_export_event_is_error() {
        assert!(ExportEvent::CrcError.is_error());
        assert!(ExportEvent::ExportFailed.is_error());
        assert!(!ExportEvent::BlobExported.is_error());
        assert!(!ExportEvent::ExportCompleted.is_error());
    }

    #[test]
    fn test_export_event_from_u8() {
        assert_eq!(ExportEvent::from_u8(0), ExportEvent::ExportStarted);
        assert_eq!(ExportEvent::from_u8(10), ExportEvent::CrcError);
        assert_eq!(ExportEvent::from_u8(200), ExportEvent::Unknown);
    }

    #[test]
    fn test_audit_entry_size() {
        assert_eq!(core::mem::size_of::<ExportAuditEntry>(), 64);
    }

    #[test]
    fn test_session_config_default() {
        let cfg = ExportSessionConfig::default(7);
        assert_eq!(cfg.session_id, 7);
        assert!(cfg.verify_crc);
    }

    #[test]
    fn test_session_config_strict() {
        let cfg = ExportSessionConfig::strict(3);
        assert!(cfg.verify_blob_ids);
        assert!(cfg.verify_crc);
    }

    #[test]
    fn test_global_export_audit_accessible() {
        EXPORT_AUDIT.log_event(0, ExportEvent::Unknown);
        let t = EXPORT_AUDIT.total_events();
        assert!(t > 0);
        EXPORT_AUDIT.reset();
    }
}
