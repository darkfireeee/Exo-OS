//! audit_export.rs — ExoFS audit log export to external formats.
//!
//! Exports ring-buffer entries to raw byte blocks,
//! filtered slices, or compact textual representations (ASCII, no_std).
//!
//! Applied rules:
//!  - OOM-02   : systematic try_reserve
//!  - ARITH-02 : checked arithmetic
//!  - RECUR-01 : zero recursion

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use super::audit_entry::{AuditEntry, AuditOp, AuditResult, AuditSeverity, AUDIT_ENTRY_SIZE};
use super::audit_filter::AuditFilter;
use super::audit_log::{AuditLog, AUDIT_LOG};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum export size in number of entries.
pub const EXPORT_MAX_ENTRIES: usize = 16384;

/// Line prefix for text export.
const TEXT_LINE_PREFIX: &str = "[AUDIT]";

// ─────────────────────────────────────────────────────────────────────────────
// ExportFormat — output format
// ─────────────────────────────────────────────────────────────────────────────

/// Export output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// Raw binary blocks (concatenation of AUDIT_ENTRY_SIZE byte entries).
    Raw,
    /// Compact textual serialization (ASCII, one entry per line).
    Text,
}

// ─────────────────────────────────────────────────────────────────────────────
// ExportRange — entry selection
// ─────────────────────────────────────────────────────────────────────────────

/// Entry selection to export.
#[derive(Clone, Debug)]
pub enum ExportRange {
    /// The last N entries.
    LastN(usize),
    /// Sequence number range (inclusive).
    SeqRange(u64, u64),
    /// All available entries.
    All,
}

// ─────────────────────────────────────────────────────────────────────────────
// ExportStats
// ─────────────────────────────────────────────────────────────────────────────

/// Export statistics.
#[derive(Clone, Debug, Default)]
pub struct ExportStats {
    pub n_exported: u32,
    pub n_filtered: u32,
    pub n_invalid: u32,
    pub byte_size: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// ExportResult — full export result
// ─────────────────────────────────────────────────────────────────────────────

/// Result of an export.
pub struct ExportResult {
    pub data: Vec<u8>,
    pub stats: ExportStats,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditExporter
// ─────────────────────────────────────────────────────────────────────────────

/// Audit log exporter.
pub struct AuditExporter {
    /// Optional filter applied before export.
    filter: Option<AuditFilter>,
    /// Total entries exported since creation.
    total_exported: u64,
}

impl AuditExporter {
    /// Creates an exporter without filter.
    pub fn new() -> Self {
        AuditExporter {
            filter: None,
            total_exported: 0,
        }
    }

    /// Creates an exporter with an applied filter.
    pub fn with_filter(filter: AuditFilter) -> Self {
        AuditExporter {
            filter: Some(filter),
            total_exported: 0,
        }
    }

    /// Replaces the filter.
    pub fn set_filter(&mut self, f: AuditFilter) {
        self.filter = Some(f);
    }

    /// Removes the filter.
    pub fn clear_filter(&mut self) {
        self.filter = None;
    }

    /// Total number of exported entries.
    pub fn total_exported(&self) -> u64 {
        self.total_exported
    }

    // ── Main export ──────────────────────────────────────────────────────────

    /// Exports entries according to the given range and format.
    pub fn export(
        &mut self,
        range: &ExportRange,
        format: ExportFormat,
    ) -> ExofsResult<ExportResult> {
        self.export_from(&AUDIT_LOG, range, format)
    }

    /// Variant with explicit log (useful for tests).
    pub fn export_from(
        &mut self,
        log: &AuditLog,
        range: &ExportRange,
        format: ExportFormat,
    ) -> ExofsResult<ExportResult> {
        let entries = self.collect_range(log, range)?;
        let result = match format {
            ExportFormat::Raw => self.export_raw(&entries)?,
            ExportFormat::Text => self.export_text(&entries)?,
        };
        self.total_exported = self
            .total_exported
            .wrapping_add(result.stats.n_exported as u64);
        Ok(result)
    }

    // ── Collection helpers ───────────────────────────────────────────────────

    fn collect_range(&self, log: &AuditLog, range: &ExportRange) -> ExofsResult<Vec<AuditEntry>> {
        let avail = log.available();
        let head = log.next_seq() as usize;

        let (start_pos, n) = match range {
            ExportRange::All => {
                let n = avail.min(EXPORT_MAX_ENTRIES);
                (head.wrapping_sub(avail), n)
            }
            ExportRange::LastN(n) => {
                let n = (*n).min(avail).min(EXPORT_MAX_ENTRIES);
                (head.wrapping_sub(n), n)
            }
            ExportRange::SeqRange(s, e) => {
                let s = *s as usize;
                let e = (*e as usize).min(head.wrapping_sub(1));
                if e < s {
                    return Ok(Vec::new());
                }
                let n = (e - s)
                    .checked_add(1)
                    .ok_or(ExofsError::OffsetOverflow)?
                    .min(EXPORT_MAX_ENTRIES);
                (s, n)
            }
        };

        let mut entries: Vec<AuditEntry> = Vec::new();
        entries.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;

        for i in 0..n {
            let pos = start_pos.wrapping_add(i);
            let e = log.read_at(pos);
            if e.is_valid() {
                entries.push(e);
            }
        }
        Ok(entries)
    }

    // ── Formats ──────────────────────────────────────────────────────────────

    fn export_raw(&self, entries: &[AuditEntry]) -> ExofsResult<ExportResult> {
        let mut stats = ExportStats::default();
        let mut data: Vec<u8> = Vec::new();

        for e in entries {
            if !e.is_valid() {
                stats.n_invalid += 1;
                continue;
            }
            if let Some(f) = &self.filter {
                if !f.matches(e) {
                    stats.n_filtered += 1;
                    continue;
                }
            }
            data.try_reserve(AUDIT_ENTRY_SIZE)
                .map_err(|_| ExofsError::NoMemory)?;
            data.extend_from_slice(e.as_bytes());
            stats.n_exported = stats.n_exported.wrapping_add(1);
        }

        stats.byte_size = data.len();
        Ok(ExportResult { data, stats })
    }

    /// Exports in ASCII text format:
    /// `[AUDIT] seq=<N> tick=<T> uid=<U> op=<OP> result=<R> sev=<S>`
    fn export_text(&self, entries: &[AuditEntry]) -> ExofsResult<ExportResult> {
        let mut stats = ExportStats::default();
        let mut data: Vec<u8> = Vec::new();

        for e in entries {
            if !e.is_valid() {
                stats.n_invalid += 1;
                continue;
            }
            if let Some(f) = &self.filter {
                if !f.matches(e) {
                    stats.n_filtered += 1;
                    continue;
                }
            }

            let op_name = AuditOp::from_u8(e.op)
                .map(|o| o.name())
                .unwrap_or("UNKNOWN");
            let res_name = match AuditResult::from_u8(e.result) {
                Some(AuditResult::Success) => "OK",
                Some(AuditResult::Denied) => "DENIED",
                Some(AuditResult::Error) => "ERROR",
                Some(AuditResult::Partial) => "PARTIAL",
                Some(AuditResult::Timeout) => "TIMEOUT",
                None => "?",
            };
            let sev_name = match AuditSeverity::from_u8(e.severity) {
                Some(AuditSeverity::Info) => "INFO",
                Some(AuditSeverity::Warning) => "WARN",
                Some(AuditSeverity::Critical) => "CRIT",
                Some(AuditSeverity::Alert) => "ALERT",
                None => "?",
            };

            let line = format_entry_line(
                e.seq,
                e.tick,
                e.actor_uid,
                op_name,
                res_name,
                sev_name,
                e.object_id,
            );
            let line_bytes = line.as_bytes();
            data.try_reserve(line_bytes.len())
                .map_err(|_| ExofsError::NoMemory)?;
            data.extend_from_slice(line_bytes);
            stats.n_exported = stats.n_exported.wrapping_add(1);
        }

        stats.byte_size = data.len();
        Ok(ExportResult { data, stats })
    }

    // ── Shortcuts ────────────────────────────────────────────────────────────

    /// Exports all entries in raw binary.
    pub fn export_all_raw(&mut self) -> ExofsResult<ExportResult> {
        self.export(&ExportRange::All, ExportFormat::Raw)
    }

    /// Exports the last N entries in text.
    pub fn export_last_n_text(&mut self, n: usize) -> ExofsResult<ExportResult> {
        self.export(&ExportRange::LastN(n), ExportFormat::Text)
    }

    /// Exports a sequence range in text.
    pub fn export_seq_range_text(&mut self, from: u64, to: u64) -> ExofsResult<ExportResult> {
        self.export(&ExportRange::SeqRange(from, to), ExportFormat::Text)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Text formatting (no_std, no complex format!)
// ─────────────────────────────────────────────────────────────────────────────

/// Formats a compact text entry line.
///
/// Format: `[AUDIT] seq=<N> tick=<T> uid=<U> obj=<O> op=<OP> res=<R> sev=<S>\n`
fn format_entry_line(
    seq: u64,
    tick: u64,
    uid: u64,
    op: &str,
    result: &str,
    severity: &str,
    obj: u64,
) -> String {
    let mut s = String::new();
    s.push_str(TEXT_LINE_PREFIX);
    s.push(' ');
    push_kv_u64(&mut s, "seq", seq);
    push_kv_u64(&mut s, "tick", tick);
    push_kv_u64(&mut s, "uid", uid);
    push_kv_u64(&mut s, "obj", obj);
    s.push_str("op=");
    s.push_str(op);
    s.push(' ');
    s.push_str("res=");
    s.push_str(result);
    s.push(' ');
    s.push_str("sev=");
    s.push_str(severity);
    s.push('\n');
    s
}

fn push_kv_u64(s: &mut String, key: &str, val: u64) {
    s.push_str(key);
    s.push('=');
    push_u64_decimal(s, val);
    s.push(' ');
}

/// Converts `n` to ASCII decimal without alloc.
fn push_u64_decimal(s: &mut String, mut n: u64) {
    if n == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut len = 0usize;
    while n > 0 {
        buf[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    // Reverse.
    for i in (0..len).rev() {
        s.push(buf[i] as char);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::audit_entry::{AuditEntry, AuditOp, AuditResult};
    use super::super::audit_filter::FilterCriteria;
    use super::super::audit_log::AuditLog;
    use super::*;

    fn push_n(log: &AuditLog, n: usize) {
        for i in 0..n {
            log.push(AuditEntry::new(
                i as u64,
                1,
                0,
                i as u64,
                [0u8; 32],
                AuditOp::Read,
                AuditResult::Success,
                log.next_seq(),
            ));
        }
    }

    #[test]
    fn test_export_raw_byte_size() {
        let log = AuditLog::new_const();
        push_n(&log, 3);
        let mut exp = AuditExporter::new();
        let res = exp
            .export_from(&log, &ExportRange::All, ExportFormat::Raw)
            .test_unwrap();
        assert_eq!(
            res.stats.byte_size,
            res.stats.n_exported as usize * AUDIT_ENTRY_SIZE
        );
    }

    #[test]
    fn test_export_text_contains_audit_prefix() {
        let log = AuditLog::new_const();
        push_n(&log, 2);
        let mut exp = AuditExporter::new();
        let res = exp
            .export_from(&log, &ExportRange::LastN(2), ExportFormat::Text)
            .test_unwrap();
        let text = core::str::from_utf8(&res.data).test_unwrap();
        assert!(
            text.contains("[AUDIT]"),
            "expected [AUDIT] in {:?}",
            &text[..text.len().min(80)]
        );
    }

    #[test]
    fn test_export_last_n_respects_limit() {
        let log = AuditLog::new_const();
        push_n(&log, 5);
        let mut exp = AuditExporter::new();
        let res = exp
            .export_from(&log, &ExportRange::LastN(2), ExportFormat::Raw)
            .test_unwrap();
        assert!(res.stats.n_exported <= 2);
    }

    #[test]
    fn test_export_with_filter() {
        let log = AuditLog::new_const();
        for i in 0..4u64 {
            let op = if i % 2 == 0 {
                AuditOp::Write
            } else {
                AuditOp::Read
            };
            log.push(AuditEntry::new(
                i,
                1,
                0,
                i,
                [0u8; 32],
                op,
                AuditResult::Success,
                i,
            ));
        }
        let filter = AuditFilter::new(FilterCriteria::by_op(AuditOp::Write));
        let mut exp = AuditExporter::with_filter(filter);
        let res = exp
            .export_from(&log, &ExportRange::All, ExportFormat::Raw)
            .test_unwrap();
        assert_eq!(res.stats.n_exported, 2);
    }

    #[test]
    fn test_export_empty_log() {
        let log = AuditLog::new_const();
        let mut exp = AuditExporter::new();
        let res = exp
            .export_from(&log, &ExportRange::All, ExportFormat::Raw)
            .test_unwrap();
        assert_eq!(res.stats.n_exported, 0);
        assert!(res.data.is_empty());
    }

    #[test]
    fn test_total_exported_increments() {
        let log = AuditLog::new_const();
        push_n(&log, 2);
        let mut exp = AuditExporter::new();
        exp.export_from(&log, &ExportRange::All, ExportFormat::Raw)
            .test_unwrap();
        assert!(exp.total_exported() >= 2);
    }

    #[test]
    fn test_push_u64_decimal_zero() {
        let mut s = String::new();
        push_u64_decimal(&mut s, 0);
        assert_eq!(s, "0");
    }

    #[test]
    fn test_push_u64_decimal_one() {
        let mut s = String::new();
        push_u64_decimal(&mut s, 1);
        assert_eq!(s, "1");
    }

    #[test]
    fn test_push_u64_decimal_large() {
        let mut s = String::new();
        push_u64_decimal(&mut s, 123456789);
        assert_eq!(s, "123456789");
    }

    #[test]
    fn test_seq_range_export() {
        let log = AuditLog::new_const();
        push_n(&log, 4);
        let start = log.next_seq().saturating_sub(3);
        let end = log.next_seq().saturating_sub(2);
        let mut exp = AuditExporter::new();
        let res = exp
            .export_from(&log, &ExportRange::SeqRange(start, end), ExportFormat::Raw)
            .test_unwrap();
        let _ = res; // Do not panic.
    }

    #[test]
    fn test_clear_filter() {
        let mut exp = AuditExporter::with_filter(AuditFilter::passthrough());
        exp.clear_filter();
        assert!(exp.filter.is_none());
    }
}
