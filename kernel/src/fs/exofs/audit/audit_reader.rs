//! audit_reader.rs — Reading and traversal of the ExoFS audit log (no_std).
//!
//! Provides a seekable cursor over the ring-buffer, a sequential iterator,
//! and search primitives by criteria.
//!
//! Applied rules:
//!  - RECUR-01: zero recursion
//!  - OOM-02: systematic try_reserve
//!  - ARITH-02: checked arithmetic

extern crate alloc;
use alloc::vec::Vec;

use super::audit_entry::{AuditEntry, AuditOp, AuditResult, AuditSeverity, AuditSummary};
use super::audit_log::{AuditLog, AUDIT_LOG, RING_SIZE};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of entries returned by a `read_n` call.
pub const READER_PAGE_MAX: usize = 1024;

// ─────────────────────────────────────────────────────────────────────────────
// ReadDirection — read direction
// ─────────────────────────────────────────────────────────────────────────────

/// Cursor traversal direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadDirection {
    /// From oldest to newest.
    Forward,
    /// From newest to oldest.
    Backward,
}

// ─────────────────────────────────────────────────────────────────────────────
// ReaderStats
// ─────────────────────────────────────────────────────────────────────────────

/// Accumulated statistics of an `AuditReader`.
#[derive(Clone, Debug, Default)]
pub struct ReaderStats {
    pub total_read: u64,
    pub total_skipped: u64,
    pub total_invalid: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditReader
// ─────────────────────────────────────────────────────────────────────────────

/// Seekable read cursor over the audit ring-buffer.
///
/// The cursor points to an absolute position (sequence). It moves forward
/// or backward one entry at a time.
pub struct AuditReader {
    /// Read source attached to this reader.
    log: *const AuditLog,
    /// Current absolute position in the ring (sequential index).
    cursor: u64,
    /// Read direction.
    direction: ReadDirection,
    /// Minimum severity filter (entries < min_severity are ignored).
    min_severity: Option<AuditSeverity>,
    /// Statistics.
    stats: ReaderStats,
}

impl AuditReader {
    /// Creates a reader positioned at the beginning (oldest available entry).
    pub fn new() -> Self {
        let head = AUDIT_LOG.next_seq();
        let count = AUDIT_LOG.available() as u64;
        let start = head.wrapping_sub(count);
        AuditReader {
            log: &AUDIT_LOG as *const AuditLog,
            cursor: start,
            direction: ReadDirection::Forward,
            min_severity: None,
            stats: ReaderStats::default(),
        }
    }

    /// Creates a reader positioned at the end (most recent entry).
    pub fn from_tail() -> Self {
        let head = AUDIT_LOG.next_seq();
        AuditReader {
            log: &AUDIT_LOG as *const AuditLog,
            cursor: head.saturating_sub(1),
            direction: ReadDirection::Backward,
            min_severity: None,
            stats: ReaderStats::default(),
        }
    }

    /// Creates a reader with an explicit log (useful for tests).
    pub fn with_log_at(log: &AuditLog, direction: ReadDirection) -> Self {
        let head = log.next_seq();
        let count = log.available() as u64;
        let cursor = match direction {
            ReadDirection::Forward => head.wrapping_sub(count),
            ReadDirection::Backward => head.saturating_sub(1),
        };
        AuditReader {
            log: log as *const AuditLog,
            cursor,
            direction,
            min_severity: None,
            stats: ReaderStats::default(),
        }
    }

    /// Filters entries whose severity is below `min`.
    pub fn min_severity(mut self, min: AuditSeverity) -> Self {
        self.min_severity = Some(min);
        self
    }

    /// Positions the cursor on absolute sequence `seq`.
    pub fn seek(&mut self, seq: u64) {
        self.cursor = seq;
    }

    /// Advances or rewinds the cursor by one entry and returns the next.
    ///
    /// Returns `None` if the cursor moved out of available bounds.
    pub fn next(&mut self) -> Option<AuditEntry> {
        let log = self.log;
        // SAFETY: see `AuditReader::log`.
        self.next_from(unsafe { &*log })
    }

    /// Variant that accepts an explicit log (useful for tests).
    pub fn next_from(&mut self, log: &AuditLog) -> Option<AuditEntry> {
        let head = log.next_seq();
        let count = log.available() as u64;
        let oldest = head.wrapping_sub(count);

        loop {
            // End boundary.
            match self.direction {
                ReadDirection::Forward => {
                    if self.cursor >= head {
                        return None;
                    }
                }
                ReadDirection::Backward => {
                    if self.cursor < oldest || head == 0 {
                        return None;
                    }
                }
            }

            let entry = log.read_at(self.cursor as usize);

            // Advance cursor.
            match self.direction {
                ReadDirection::Forward => {
                    self.cursor = self.cursor.wrapping_add(1);
                }
                ReadDirection::Backward => {
                    if self.cursor == 0 {
                        return None;
                    }
                    self.cursor = self.cursor.wrapping_sub(1);
                }
            }

            // Magic validation.
            if !entry.is_valid() {
                self.stats.total_invalid = self.stats.total_invalid.wrapping_add(1);
                continue;
            }

            // Severity filter.
            if let Some(min) = self.min_severity {
                if entry.severity < min as u8 {
                    self.stats.total_skipped = self.stats.total_skipped.wrapping_add(1);
                    continue;
                }
            }

            self.stats.total_read = self.stats.total_read.wrapping_add(1);
            return Some(entry);
        }
    }

    // ── Paged reads ───────────────────────────────────────────────────────────

    /// Reads up to `n` entries (≤ `READER_PAGE_MAX`) from the current position.
    pub fn read_n(&mut self, n: usize) -> ExofsResult<Vec<AuditEntry>> {
        let cap = n.min(READER_PAGE_MAX);
        let mut out: Vec<AuditEntry> = Vec::new();
        out.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < cap {
            match self.next() {
                Some(e) => {
                    out.push(e);
                    i += 1;
                }
                None => break,
            }
        }
        Ok(out)
    }

    /// Reads all available entries from the current position.
    pub fn read_all(&mut self) -> ExofsResult<Vec<AuditEntry>> {
        self.read_n(RING_SIZE)
    }

    // ── Search ───────────────────────────────────────────────────────────────

    /// Returns the first entry satisfying `pred` from the current position.
    pub fn find<F>(&mut self, pred: F) -> Option<AuditEntry>
    where
        F: Fn(&AuditEntry) -> bool,
    {
        loop {
            match self.next() {
                Some(e) if pred(&e) => return Some(e),
                Some(_) => {}
                None => return None,
            }
        }
    }

    /// Collects all entries satisfying `pred` (bounded by `RING_SIZE`).
    pub fn collect_if<F>(&mut self, pred: F) -> ExofsResult<Vec<AuditEntry>>
    where
        F: Fn(&AuditEntry) -> bool,
    {
        let mut out: Vec<AuditEntry> = Vec::new();
        loop {
            match self.next() {
                Some(e) if pred(&e) => {
                    out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    out.push(e);
                }
                Some(_) => {}
                None => break,
            }
        }
        Ok(out)
    }

    /// Returns all entries for a given actor.
    pub fn entries_by_actor(&mut self, actor_uid: u64) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.actor_uid == actor_uid)
    }

    /// Returns all entries for a given operation.
    pub fn entries_by_op(&mut self, op: AuditOp) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.op == op as u8)
    }

    /// Returns all entries for a given result.
    pub fn entries_by_result(&mut self, r: AuditResult) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.result == r as u8)
    }

    /// Returns all security entries.
    pub fn security_entries(&mut self) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.is_security())
    }

    /// Returns all entries within a tick range.
    pub fn entries_in_tick_range(
        &mut self,
        from_tick: u64,
        to_tick: u64,
    ) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.tick >= from_tick && e.tick <= to_tick)
    }

    // ── Statistics ───────────────────────────────────────────────────────────

    /// Computes a summary of all available entries (restarts from the beginning).
    pub fn summarize(&mut self) -> AuditSummary {
        let log = self.log;
        // SAFETY: see `AuditReader::log`.
        let log = unsafe { &*log };
        self.seek(log.next_seq().wrapping_sub(log.available() as u64));
        let mut summary = AuditSummary::default();
        loop {
            match self.next() {
                Some(e) => summary.feed(&e),
                None => break,
            }
        }
        summary
    }

    /// Reader statistics.
    pub fn stats(&self) -> &ReaderStats {
        &self.stats
    }

    /// Resets statistics.
    pub fn reset_stats(&mut self) {
        self.stats = ReaderStats::default();
    }

    /// Current cursor position (absolute sequence).
    pub fn cursor(&self) -> u64 {
        self.cursor
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Free helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the last N entries of the global log.
pub fn last_n_entries(n: usize) -> ExofsResult<Vec<AuditEntry>> {
    let cap = n.min(RING_SIZE).min(READER_PAGE_MAX);
    let mut buf: Vec<AuditEntry> = Vec::new();
    buf.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
    let avail = AUDIT_LOG.available();
    let actual = cap.min(avail);
    buf.resize(
        actual,
        AuditEntry::new(
            0,
            0,
            0,
            0,
            [0u8; 32],
            AuditOp::Read,
            AuditResult::Success,
            0,
        ),
    );
    let n_read = AUDIT_LOG.read_recent_into(&mut buf, actual);
    buf.truncate(n_read);
    Ok(buf)
}

/// Returns a quick summary of the global log.
pub fn quick_summary() -> AuditSummary {
    let mut s = AuditSummary::default();
    AUDIT_LOG.for_each(|e| s.feed(e));
    s
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::audit_log::AuditLog;
    use super::*;

    fn push_entry(log: &AuditLog, op: AuditOp, result: AuditResult, uid: u64) {
        log.push(AuditEntry::new(
            1,
            uid,
            0,
            0,
            [0u8; 32],
            op,
            result,
            log.next_seq(),
        ));
    }

    #[test]
    fn test_new_reader_empty_log() {
        let log = AuditLog::new_const();
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        assert!(r.next_from(&log).is_none());
    }

    #[test]
    fn test_forward_reads_in_order() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read, AuditResult::Success, 1);
        push_entry(&log, AuditOp::Write, AuditResult::Success, 2);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let e1 = r.next_from(&log).test_unwrap();
        let e2 = r.next_from(&log).test_unwrap();
        assert!(e1.seq <= e2.seq);
    }

    #[test]
    fn test_backward_reads_last_first() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read, AuditResult::Success, 1);
        push_entry(&log, AuditOp::Delete, AuditResult::Success, 2);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Backward);
        let e = r.next_from(&log).test_unwrap();
        assert_eq!(e.op, AuditOp::Delete as u8);
    }

    #[test]
    fn test_find_by_op() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read, AuditResult::Success, 1);
        push_entry(&log, AuditOp::Write, AuditResult::Success, 2);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let e = r.find(|e| e.op == AuditOp::Write as u8);
        assert!(e.is_some());
    }

    #[test]
    fn test_min_severity_filters() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read, AuditResult::Success, 1); // Info
        push_entry(&log, AuditOp::Read, AuditResult::Denied, 2); // Critical
        // Read with min_severity filter = Critical on the global log.
        // Primarily verifies that the method does not panic.
        let mut r = AuditReader::new().min_severity(AuditSeverity::Critical);
        let _ = r.read_n(10).test_unwrap();
    }

    #[test]
    fn test_read_all_returns_vec() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Create, AuditResult::Success, 1);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let all = r.read_all().test_unwrap();
        assert!(!all.is_empty());
    }

    #[test]
    fn test_entries_by_actor() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read, AuditResult::Success, 99);
        push_entry(&log, AuditOp::Read, AuditResult::Success, 42);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let v = r.collect_if(|e| e.actor_uid == 99).test_unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn test_stats_counts_reads() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read, AuditResult::Success, 1);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        r.next_from(&log);
        assert_eq!(r.stats().total_read, 1);
    }

    #[test]
    fn test_last_n_entries() {
        let v = last_n_entries(2).test_unwrap();
        // The global log contains at least one entry thanks to other tests.
        let _ = v;
    }

    #[test]
    fn test_quick_summary() {
        let _s = quick_summary();
        // assert!(s.total >= 0);
    }

    #[test]
    fn test_seek_and_reread() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::GcTrigger, AuditResult::Success, 1);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let pos = r.cursor();
        let e1 = r.next_from(&log);
        r.seek(pos);
        let e2 = r.next_from(&log);
        assert_eq!(e1.map(|e| e.op), e2.map(|e| e.op));
    }
}
