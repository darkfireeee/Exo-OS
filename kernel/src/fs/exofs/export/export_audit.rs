//! export_audit.rs — Real-time audit log of export/import operations
//! (no_std, Ring0, thread-safe via AtomicU64 + UnsafeCell).
//!
//! This module provides:
//!  - `ExportAuditLog`     : circular ring of 512 audit entries.
//!  - `ExportEvent`        : enumeration of all traceable events.
//!  - `ExportAuditEntry`   : 64-byte structure per audit entry.
//!  - `ExportAuditStats`   : aggregated counters per event type.
//!  - `ExportAuditQuery`   : querying the last N entries.
//!  - `ExportSession`      : export/import session with state tracking.
//!  - `ExportSessionConfig`: session configuration parameters.
//!
//! RECUR-01 : no recursion — while/for loops.
//! ARITH-02 : saturating_* / wrapping_* on counters.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

// ─── Constants ───────────────────────────────────────────────────────────────
/// Capacity of circular ring (power of 2 for masking).
pub const EXPORT_AUDIT_RING: usize = 512;
const RING_MASK: usize = EXPORT_AUDIT_RING - 1;

// ─── Global Singleton ────────────────────────────────────────────────────────
/// Global audit log of export module.
pub static EXPORT_AUDIT: ExportAuditLog = ExportAuditLog::new_const();

// ─── Audit Events ────────────────────────────────────────────────────────────

/// Traceable events in the export audit log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExportEvent {
    /// Export session started.
    ExportStarted = 0,
    /// Export session completed successfully.
    ExportCompleted = 1,
    /// Export session aborted due to error.
    ExportFailed = 2,
    /// Import session started.
    ImportStarted = 3,
    /// Import session completed successfully.
    ImportCompleted = 4,
    /// Import session aborted due to error.
    ImportFailed = 5,
    /// Blob exported to archive.
    BlobExported = 6,
    /// Blob imported from archive.
    BlobImported = 7,
    /// Tombstone exported.
    TombstoneExported = 8,
    /// Tombstone imported.
    TombstoneImported = 9,
    /// CRC32C error detected on a payload.
    CrcError = 10,
    /// BlobId does not match data hash (RULE 11).
    BlobIdMismatch = 11,
    /// Invalid magic detected in archive (RULE 8).
    MagicError = 12,
    /// Blob conflict resolved (blob already present).
    ConflictResolved = 13,
    /// Entry skipped (filtered by policy).
    EntrySkipped = 14,
    /// Archive compressed.
    CompressionApplied = 15,
    /// Archive decompressed.
    DecompressionDone = 16,
    /// Integrity verification succeeded.
    IntegrityVerified = 17,
    /// Integrity verification failed.
    IntegrityFailed = 18,
    /// Recovery checkpoint saved.
    CheckpointSaved = 19,
    /// Export/import session started (generic alias).
    SessionStarted = 20,
    /// Export/import session completed successfully (generic alias).
    SessionCompleted = 21,
    /// Export/import session aborted due to error (generic alias).
    SessionFailed = 22,
    /// Incremental export performed.
    IncrementalExport = 23,
    /// Unknown / reserved event.
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

    /// Returns true if the event represents an error.
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

    /// Returns true if the event concerns an individual blob.
    pub fn is_blob_event(&self) -> bool {
        matches!(
            self,
            Self::BlobExported | Self::BlobImported | Self::BlobIdMismatch
        )
    }
}

// ─── Audit Entry ─────────────────────────────────────────────────────────────

/// Audit entry: 64 bytes, aligned and copyable.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ExportAuditEntry {
    /// Event type.
    pub event: ExportEvent,
    /// Padding after enum.
    pub _pad: [u8; 3],
    /// Session identifier (session_id).
    pub session_id: u32,
    /// Monotonic sequence (global sequence number).
    pub seq: u64,
    /// Epoch or timestamp (depending on context).
    pub epoch: u64,
    /// Associated BlobId (first 8 bytes for compactness).
    pub blob_id_prefix: [u8; 8],
    /// Additional BlobId bytes.
    pub blob_id_mid: [u8; 16],
    /// Size of payload involved (0 if not applicable).
    pub payload_size: u64,
    /// Error code (0 = success).
    pub error_code: u32,
    /// Final padding to reach 64 bytes.
    pub _pad2: [u8; 4],
}

const _: () = assert!(core::mem::size_of::<ExportAuditEntry>() == 64);

impl ExportAuditEntry {
    /// Creates an empty audit entry.
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

    /// Creates an exported blob entry.
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

    /// Creates an imported blob entry.
    pub fn blob_imported(session_id: u32, blob_id: &[u8; 32], size: u64) -> Self {
        let mut e = Self::new_empty();
        e.event = ExportEvent::BlobImported;
        e.session_id = session_id;
        e.payload_size = size;
        e.blob_id_prefix.copy_from_slice(&blob_id[..8]);
        e.blob_id_mid.copy_from_slice(&blob_id[8..24]);
        e
    }

    /// Creates an error entry.
    pub fn error(session_id: u32, event: ExportEvent, code: u32) -> Self {
        let mut e = Self::new_empty();
        e.event = event;
        e.session_id = session_id;
        e.error_code = code;
        e
    }

    /// Returns true if the entry represents an error.
    pub fn is_error(&self) -> bool {
        self.event.is_error()
    }
}

// ─── Audit Statistics ────────────────────────────────────────────────────────

/// Aggregated counters of audit events.
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

    /// Returns true if no errors were recorded.
    pub fn is_clean(&self) -> bool {
        self.exports_failed == 0
            && self.imports_failed == 0
            && self.crc_errors == 0
            && self.magic_errors == 0
            && self.blob_id_mismatches == 0
    }
}

// ─── Audit Log (circular ring, thread-safe) ──────────────────────────────────

/// Audit log for the export module — circular ring, thread-safe.
/// Implemented with UnsafeCell + AtomicU64 spinlock (Ring0 pattern).
pub struct ExportAuditLog {
    entries: UnsafeCell<[ExportAuditEntry; EXPORT_AUDIT_RING]>,
    /// Write pointer (head), wrapping over EXPORT_AUDIT_RING.
    head: AtomicU64,
    /// Global sequence counter.
    seq: AtomicU64,
    /// Spinlock (0 = free, 1 = held).
    lock: AtomicU64,
    /// Integrated statistics.
    stats: UnsafeCell<ExportAuditStats>,
}

unsafe impl Sync for ExportAuditLog {}
unsafe impl Send for ExportAuditLog {}

impl ExportAuditLog {
    /// Const constructor for the static singleton.
    pub const fn new_const() -> Self {
        Self {
            entries: UnsafeCell::new([ExportAuditEntry::new_empty(); EXPORT_AUDIT_RING]),
            head: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            lock: AtomicU64::new(0),
            stats: UnsafeCell::new(ExportAuditStats::new()),
        }
    }

    /// Acquires the spinlock (busy-wait, Ring0 safe).
    fn acquire(&self) {
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    /// Releases the spinlock.
    fn release(&self) {
        self.lock.store(0, Ordering::Release);
    }

    /// Records an event in the log.
    pub fn record(&self, mut entry: ExportAuditEntry) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        entry.seq = seq;
        self.acquire();
        let head = self.head.load(Ordering::Relaxed);
        let idx = (head as usize) & RING_MASK;
        // SAFETY: safety invariant verified by the caller function preconditions.
        unsafe {
            let entries = &mut *self.entries.get();
            entries[idx] = entry;
            let stats = &mut *self.stats.get();
            stats.record(entry.event);
        }
        self.head.store(head.wrapping_add(1), Ordering::Relaxed);
        self.release();
    }

    /// Records a simple event (without BlobId).
    pub fn log_event(&self, session_id: u32, event: ExportEvent) {
        let mut entry = ExportAuditEntry::new_empty();
        entry.event = event;
        entry.session_id = session_id;
        self.record(entry);
    }

    /// Records an exported blob.
    pub fn log_blob_exported(&self, session_id: u32, blob_id: &[u8; 32], size: u64, epoch: u64) {
        let entry = ExportAuditEntry::blob_exported(session_id, blob_id, size, epoch);
        self.record(entry);
    }

    /// Records an imported blob.
    pub fn log_blob_imported(&self, session_id: u32, blob_id: &[u8; 32], size: u64) {
        let entry = ExportAuditEntry::blob_imported(session_id, blob_id, size);
        self.record(entry);
    }

    /// Records an error.
    pub fn log_error(&self, session_id: u32, event: ExportEvent, code: u32) {
        let entry = ExportAuditEntry::error(session_id, event, code);
        self.record(entry);
    }

    /// Returns a copy of current statistics.
    pub fn stats(&self) -> ExportAuditStats {
        self.acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired prior.
        let s = unsafe { *self.stats.get() };
        self.release();
        s
    }

    /// Returns total number of recorded events.
    pub fn total_events(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Copies the last N entries into `out` (RECUR-01: while loop).
    /// Returns the number of copied entries.
    pub fn last_n(&self, out: &mut [ExportAuditEntry]) -> usize {
        if out.is_empty() {
            return 0;
        }
        self.acquire();
        let head = self.head.load(Ordering::Relaxed) as usize;
        let total = self.seq.load(Ordering::Relaxed) as usize;
        let available = total.min(EXPORT_AUDIT_RING);
        let n = out.len().min(available);
        // SAFETY: exclusive access guaranteed by atomic lock acquired prior.
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

    /// Returns the last recorded entry, or None if the log is empty.
    pub fn last_entry(&self) -> Option<ExportAuditEntry> {
        if self.seq.load(Ordering::Relaxed) == 0 {
            return None;
        }
        self.acquire();
        let head = self.head.load(Ordering::Relaxed) as usize;
        let idx = (head.wrapping_sub(1)) & RING_MASK;
        // SAFETY: exclusive access guaranteed by atomic lock acquired prior.
        let entry = unsafe { (*self.entries.get())[idx] };
        self.release();
        Some(entry)
    }

    /// Returns the number of errors among the last `n` entries.
    /// RECUR-01: while loop.
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

    /// Resets the log and statistics to zero.
    pub fn reset(&self) {
        self.acquire();
        // SAFETY: safety invariant verified by the caller function preconditions.
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

    /// Returns true if no error is present in current statistics.
    pub fn is_clean(&self) -> bool {
        self.stats().is_clean()
    }
}

// ─── Export/Import Session ───────────────────────────────────────────────────

/// State of an export or import session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Running,
    Completed,
    Failed,
}

/// Export/import session with duration and state tracking.
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
    /// Creates a new export session.
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

    /// Creates a new import session.
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

    /// Starts the session.
    pub fn start(&mut self, audit: &ExportAuditLog) {
        self.state = SessionState::Running;
        let event = if self.is_import {
            ExportEvent::ImportStarted
        } else {
            ExportEvent::ExportStarted
        };
        audit.log_event(self.session_id, event);
    }

    /// Records a processed blob.
    pub fn record_blob(&mut self, blob_id: &[u8; 32], size: u64, audit: &ExportAuditLog) {
        self.blobs_processed = self.blobs_processed.saturating_add(1);
        self.bytes_processed = self.bytes_processed.saturating_add(size);
        if self.is_import {
            audit.log_blob_imported(self.session_id, blob_id, size);
        } else {
            audit.log_blob_exported(self.session_id, blob_id, size, self.epoch_start);
        }
    }

    /// Records an error.
    pub fn record_error(&mut self, event: ExportEvent, code: u32, audit: &ExportAuditLog) {
        self.errors = self.errors.saturating_add(1);
        audit.log_error(self.session_id, event, code);
    }

    /// Completes the session successfully.
    pub fn complete(&mut self, audit: &ExportAuditLog) {
        self.state = SessionState::Completed;
        let event = if self.is_import {
            ExportEvent::ImportCompleted
        } else {
            ExportEvent::ExportCompleted
        };
        audit.log_event(self.session_id, event);
    }

    /// Terminates the session with failure.
    pub fn fail(&mut self, audit: &ExportAuditLog) {
        self.state = SessionState::Failed;
        let event = if self.is_import {
            ExportEvent::ImportFailed
        } else {
            ExportEvent::ExportFailed
        };
        audit.log_event(self.session_id, event);
    }

    /// Returns true if the session is finished (success or failure).
    pub fn is_done(&self) -> bool {
        matches!(self.state, SessionState::Completed | SessionState::Failed)
    }

    /// Returns true if no errors were recorded.
    pub fn is_clean(&self) -> bool {
        self.errors == 0
    }
}

// ─── Session Configuration ──────────────────────────────────────────────────

/// Configuration parameter of a session.
#[derive(Clone, Copy, Debug)]
pub struct ExportSessionConfig {
    /// Session identifier (must be unique per active session).
    pub session_id: u32,
    /// Base epoch (incremental export).
    pub epoch_base: u64,
    /// Target epoch.
    pub epoch_target: u64,
    /// Verify BlobId = blake3(data) — RULE 11.
    pub verify_blob_ids: bool,
    /// Verify payload CRC32C checksums.
    pub verify_crc: bool,
    /// Maximum number of blobs to process (0 = unlimited).
    pub max_blobs: u32,
    /// Maximum total size in bytes (0 = unlimited).
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
use crate::fs::exofs::test_support::TestUnwrapExt;
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
        let e = log.last_entry().test_expect("has entry");
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
        let e = log.last_entry().test_expect("has entry");
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
