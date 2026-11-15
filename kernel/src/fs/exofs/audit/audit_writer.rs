//! audit_writer.rs — Writing entries to the ExoFS audit log (no_std).
//!
//! Applied rules:
//!  - OOM-02   : systematic try_reserve
//!  - ARITH-02 : checked arithmetic
//!  - RECUR-01 : zero recursion

extern crate alloc;
use alloc::vec::Vec;

use super::audit_entry::{AuditEntry, AuditEntryBuilder, AuditOp, AuditResult};
use super::audit_log::AUDIT_LOG;
use crate::fs::exofs::core::clock::exofs_ticks; // DAG-01: replaces arch::time
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum capacity of the batch buffer.
pub const WRITER_BATCH_MAX: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// WriterContext — current actor context
// ─────────────────────────────────────────────────────────────────────────────

/// Authentication context of the actor emitting entries.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriterContext {
    /// Actor UID.
    pub actor_uid: u64,
    /// Actor capabilities (bitmask).
    pub actor_cap: u64,
}

impl WriterContext {
    pub fn new(actor_uid: u64, actor_cap: u64) -> Self {
        WriterContext {
            actor_uid,
            actor_cap,
        }
    }

    /// Kernel context (UID 0, full capabilities).
    pub fn kernel() -> Self {
        WriterContext {
            actor_uid: 0,
            actor_cap: u64::MAX,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WritePolicy — write policy
// ─────────────────────────────────────────────────────────────────────────────

/// Write policy for the ring-buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritePolicy {
    /// Immediate write on each call.
    Immediate,
    /// Accumulates in an internal buffer — emptied via `flush()`.
    Buffered,
}

// ─────────────────────────────────────────────────────────────────────────────
// WriteStats
// ─────────────────────────────────────────────────────────────────────────────

/// Accumulated statistics of the `AuditWriter`.
#[derive(Clone, Debug, Default)]
pub struct WriteStats {
    /// Total number of entries written to the ring.
    pub total_written: u64,
    /// Total number of batches flushed.
    pub total_flushes: u64,
    /// Number of entries dropped on error (full buffer).
    pub total_dropped: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditWriter
// ─────────────────────────────────────────────────────────────────────────────

/// Writer for audit log entries.
///
/// Supports immediate and buffered writes.  
/// Thread-safety: intended for single-thread usage (no internal locks).
pub struct AuditWriter {
    ctx: WriterContext,
    policy: WritePolicy,
    buffer: Vec<AuditEntry>,
    stats: WriteStats,
}

impl AuditWriter {
    /// Creates a writer with given context and policy.
    pub fn new(ctx: WriterContext, policy: WritePolicy) -> Self {
        AuditWriter {
            ctx,
            policy,
            buffer: Vec::new(),
            stats: WriteStats::default(),
        }
    }

    /// Creates an immediate writer for the kernel.
    pub fn kernel_immediate() -> Self {
        Self::new(WriterContext::kernel(), WritePolicy::Immediate)
    }

    /// Changes the write policy.
    pub fn set_policy(&mut self, policy: WritePolicy) {
        self.policy = policy;
    }

    // ── Simple write ──────────────────────────────────────────────────────────

    /// Records a simple operation on an object.
    ///
    /// The tick is automatically read.
    pub fn write(
        &mut self,
        object_id: u64,
        blob_id: [u8; 32],
        op: AuditOp,
        result: AuditResult,
    ) -> ExofsResult<()> {
        let entry = AuditEntryBuilder::new()
            .tick(exofs_ticks())
            .actor_uid(self.ctx.actor_uid)
            .actor_cap(self.ctx.actor_cap)
            .object_id(object_id)
            .blob_id(blob_id)
            .op(op)
            .result(result)
            .seq(AUDIT_LOG.next_seq())
            .build()?;
        self.emit(entry)
    }

    /// Records a pre-constructed entry.
    pub fn write_entry(&mut self, entry: AuditEntry) -> ExofsResult<()> {
        entry.validate()?;
        self.emit(entry)
    }

    // ── Operation shortcuts ───────────────────────────────────────────────────

    /// Records a read access.
    pub fn record_read(&mut self, object_id: u64, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.write(object_id, blob_id, AuditOp::Read, AuditResult::Success)
    }

    /// Records a successful write.
    pub fn record_write(&mut self, object_id: u64, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.write(object_id, blob_id, AuditOp::Write, AuditResult::Success)
    }

    /// Records a creation.
    pub fn record_create(&mut self, object_id: u64, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.write(object_id, blob_id, AuditOp::Create, AuditResult::Success)
    }

    /// Records a deletion.
    pub fn record_delete(&mut self, object_id: u64, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.write(object_id, blob_id, AuditOp::Delete, AuditResult::Success)
    }

    /// Records a permission denial.
    pub fn record_denied(
        &mut self,
        object_id: u64,
        blob_id: [u8; 32],
        op: AuditOp,
    ) -> ExofsResult<()> {
        self.write(object_id, blob_id, op, AuditResult::Denied)
    }

    /// Records a checksum failure.
    pub fn record_checksum_fail(&mut self, object_id: u64) -> ExofsResult<()> {
        self.write(
            object_id,
            [0u8; 32],
            AuditOp::ChecksumFail,
            AuditResult::Error,
        )
    }

    /// Records a cryptographic key event.
    pub fn record_crypto_key(&mut self, object_id: u64, result: AuditResult) -> ExofsResult<()> {
        self.write(object_id, [0u8; 32], AuditOp::CryptoKey, result)
    }

    /// Records a policy change.
    pub fn record_policy_change(&mut self, object_id: u64) -> ExofsResult<()> {
        self.write(
            object_id,
            [0u8; 32],
            AuditOp::PolicyChange,
            AuditResult::Success,
        )
    }

    // ── Batch ─────────────────────────────────────────────────────────────────

    /// Writes a batch of entries (max `WRITER_BATCH_MAX`).
    ///
    /// All entries are validated prior to writing. If an entry is
    /// invalid, the batch is cancelled and `InvalidArgument` is returned.
    pub fn write_batch(&mut self, entries: &[AuditEntry]) -> ExofsResult<u32> {
        if entries.len() > WRITER_BATCH_MAX {
            return Err(ExofsError::InvalidArgument);
        }
        for e in entries {
            e.validate()?;
        }
        let mut written = 0u32;
        for &e in entries {
            self.emit(e)?;
            written = written.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        }
        Ok(written)
    }

    // ── Flush / buffer ────────────────────────────────────────────────────────

    /// Flushes the internal buffer into the ring-buffer.
    ///
    /// Returns the number of flushed entries.
    pub fn flush(&mut self) -> u32 {
        let n = self.buffer.len();
        for entry in self.buffer.drain(..) {
            AUDIT_LOG.push(entry);
            self.stats.total_written = self.stats.total_written.wrapping_add(1);
        }
        self.stats.total_flushes = self.stats.total_flushes.wrapping_add(1);
        n as u32
    }

    /// Number of pending entries in the buffer.
    pub fn pending(&self) -> usize {
        self.buffer.len()
    }

    /// `true` if the internal buffer is empty.
    pub fn is_flushed(&self) -> bool {
        self.buffer.is_empty()
    }

    // ── Statistics ───────────────────────────────────────────────────────────

    /// Returns the current statistics.
    pub fn stats(&self) -> &WriteStats {
        &self.stats
    }

    /// Resets statistics to zero.
    pub fn reset_stats(&mut self) {
        self.stats = WriteStats::default();
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    fn emit(&mut self, entry: AuditEntry) -> ExofsResult<()> {
        match self.policy {
            WritePolicy::Immediate => {
                AUDIT_LOG.push(entry);
                self.stats.total_written = self.stats.total_written.wrapping_add(1);
            }
            WritePolicy::Buffered => {
                if self.buffer.len() >= WRITER_BATCH_MAX {
                    // Buffer full: automatic flush.
                    self.flush();
                }
                self.buffer
                    .try_reserve(1)
                    .map_err(|_| ExofsError::NoMemory)?;
                self.buffer.push(entry);
            }
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Free helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Quick write of a security event from anywhere.
pub fn write_security_event(actor_uid: u64, object_id: u64, op: AuditOp, result: AuditResult) {
    let tick = exofs_ticks();
    if let Ok(entry) = AuditEntryBuilder::new()
        .tick(tick)
        .actor_uid(actor_uid)
        .actor_cap(0)
        .object_id(object_id)
        .op(op)
        .result(result)
        .seq(AUDIT_LOG.next_seq())
        .build()
    {
        AUDIT_LOG.push(entry);
    }
}

/// Immediately records a permission denial (inline, no allocation).
pub fn record_perm_denied(actor_uid: u64, object_id: u64, op: AuditOp) {
    write_security_event(actor_uid, object_id, op, AuditResult::Denied);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;
    // use super::super::audit_log::AuditLog;

    fn writer() -> AuditWriter {
        AuditWriter::new(WriterContext::new(42, 0xFF), WritePolicy::Immediate)
    }

    #[test]
    fn test_write_read_is_ok() {
        let mut w = writer();
        w.write(1, [0u8; 32], AuditOp::Read, AuditResult::Success)
            .test_unwrap();
        let e = AUDIT_LOG.read_last().test_unwrap();
        assert!(e.is_valid());
    }

    #[test]
    fn test_record_create() {
        let mut w = writer();
        w.record_create(99, [1u8; 32]).test_unwrap();
        let e = AUDIT_LOG.read_last().test_unwrap();
        assert_eq!(e.op, AuditOp::Create as u8);
    }

    #[test]
    fn test_record_denied() {
        let mut w = writer();
        w.record_denied(1, [0u8; 32], AuditOp::Write).test_unwrap();
        let e = AUDIT_LOG.read_last().test_unwrap();
        assert_eq!(e.result, AuditResult::Denied as u8);
    }

    #[test]
    fn test_stats_increments() {
        let mut w = writer();
        w.record_read(1, [0u8; 32]).test_unwrap();
        w.record_write(2, [0u8; 32]).test_unwrap();
        assert_eq!(w.stats().total_written, 2);
    }

    #[test]
    fn test_buffered_flush() {
        let mut w = AuditWriter::new(WriterContext::kernel(), WritePolicy::Buffered);
        w.record_read(10, [0u8; 32]).test_unwrap();
        assert_eq!(w.pending(), 1);
        let n = w.flush();
        assert_eq!(n, 1);
        assert!(w.is_flushed());
    }

    #[test]
    fn test_batch_valid() {
        let mut w = writer();
        let entries: Vec<AuditEntry> = (0u64..4)
            .map(|i| {
                AuditEntry::new(
                    i,
                    1,
                    0,
                    i,
                    [0u8; 32],
                    AuditOp::Read,
                    AuditResult::Success,
                    i,
                )
            })
            .collect();
        let n = w.write_batch(&entries).test_unwrap();
        assert_eq!(n, 4);
    }

    #[test]
    fn test_batch_too_large() {
        let mut w = writer();
        let entries: Vec<AuditEntry> = (0u64..300)
            .map(|i| {
                AuditEntry::new(
                    i,
                    1,
                    0,
                    i,
                    [0u8; 32],
                    AuditOp::Read,
                    AuditResult::Success,
                    i,
                )
            })
            .collect();
        assert!(w.write_batch(&entries).is_err());
    }

    #[test]
    fn test_write_entry_invalid_magic() {
        let mut w = writer();
        let mut e = AuditEntry::new(
            1,
            1,
            0,
            1,
            [0u8; 32],
            AuditOp::Read,
            AuditResult::Success,
            0,
        );
        e.magic = 0xDEAD;
        assert!(w.write_entry(e).is_err());
    }

    #[test]
    fn test_kernel_writer() {
        let mut w = AuditWriter::kernel_immediate();
        w.record_crypto_key(0, AuditResult::Success).test_unwrap();
        let e = AUDIT_LOG.read_last().test_unwrap();
        assert_eq!(e.op, AuditOp::CryptoKey as u8);
    }

    #[test]
    fn test_write_policy_change() {
        let mut w = writer();
        w.record_policy_change(5).test_unwrap();
        let e = AUDIT_LOG.read_last().test_unwrap();
        assert!(e.is_security());
    }

    #[test]
    fn test_reset_stats() {
        let mut w = writer();
        w.record_read(1, [0u8; 32]).test_unwrap();
        w.reset_stats();
        assert_eq!(w.stats().total_written, 0);
    }

    #[test]
    fn test_write_checksum_fail() {
        let mut w = writer();
        w.record_checksum_fail(77).test_unwrap();
        let e = AUDIT_LOG.read_last().test_unwrap();
        assert_eq!(e.op, AuditOp::ChecksumFail as u8);
        assert_eq!(e.result, AuditResult::Error as u8);
    }

    #[test]
    fn test_buffered_no_flush_until_explicit() {
        let mut w = AuditWriter::new(WriterContext::kernel(), WritePolicy::Buffered);
        w.record_delete(10, [0u8; 32]).test_unwrap();
        assert_eq!(w.pending(), 1);
        assert!(!w.is_flushed());
    }

    #[test]
    fn test_write_entry_valid_roundtrip() {
        let mut w = writer();
        let e = AuditEntry::new(
            50,
            7,
            0,
            3,
            [0u8; 32],
            AuditOp::Rename,
            AuditResult::Success,
            0,
        );
        w.write_entry(e).test_unwrap();
        let last = AUDIT_LOG.read_last().test_unwrap();
        assert_eq!(last.op, AuditOp::Rename as u8);
    }
}
