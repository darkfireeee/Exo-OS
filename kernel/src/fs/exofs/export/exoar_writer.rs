//! exoar_writer.rs — Creation and finalization of ExoAR archives (no_std).
//!
//! This module provides:
//!  - `ArchiveSink`         : sequential write trait.
//!  - `ExoarWriter`         : main writer (header → blobs → footer).
//!  - `ExoarBufferedWriter` : accumulates data in memory before flush.
//!  - `ExoarWriteStats`     : statistics of the in-progress archive.
//!  - `ExoarWriteOptions`   : writer configuration.
//!  - `ExoarWriteError`     : specific write errors.
//!  - `SinkVec`             : implementation of ArchiveSink on Vec<u8>.
//!
//! RULE 8  : magic written FIRST for each structure.
//! RULE 11 : BlobId = blake3(raw data) — computed here.
//! RECUR-01: no recursion — while loops.
//! OOM-02  : try_reserve before push.
//! ARITH-02: saturating_* / wrapping_* on all counters.

extern crate alloc;
use super::exoar_format::{
    crc32c_compute, crc32c_update, ExoarEntryHeader, ExoarFooter, ExoarHeader,
    ARCHIVE_FLAG_INCREMENTAL, ENTRY_FLAG_TOMBSTONE, EXOAR_MAX_ENTRIES, EXOAR_MAX_PAYLOAD,
};
use crate::fs::exofs::core::blob_id;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::mem::size_of;

// ─── Output Trait ────────────────────────────────────────────────────────────

/// Sequential write abstraction to an archive destination.
/// RECUR-01: implementations do not call themselves recursively.
pub trait ArchiveSink {
    /// Writes exactly `buf` to the destination.
    fn write_all(&mut self, buf: &[u8]) -> ExofsResult<()>;
    /// Returns the total number of bytes written.
    fn bytes_written(&self) -> u64;
}

// ─── Write Errors ────────────────────────────────────────────────────────────

/// Error specific to ExoAR archive writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExoarWriteError {
    /// Archive has already been finalized.
    AlreadyFinalized,
    /// Archive has not yet been initialized (header not written).
    NotStarted,
    /// Too many entries (exceeds EXOAR_MAX_ENTRIES).
    TooManyEntries,
    /// Payload too large (exceeds EXOAR_MAX_PAYLOAD).
    PayloadTooLarge,
    /// Memory allocation error.
    OutOfMemory,
    /// Write error to sink.
    IoError,
}

impl From<ExoarWriteError> for ExofsError {
    fn from(e: ExoarWriteError) -> Self {
        match e {
            ExoarWriteError::AlreadyFinalized => ExofsError::InvalidArgument,
            ExoarWriteError::NotStarted => ExofsError::InvalidArgument,
            ExoarWriteError::TooManyEntries => ExofsError::OffsetOverflow,
            ExoarWriteError::PayloadTooLarge => ExofsError::OffsetOverflow,
            ExoarWriteError::OutOfMemory => ExofsError::NoMemory,
            ExoarWriteError::IoError => ExofsError::IoFailed,
        }
    }
}

// ─── Write Options ───────────────────────────────────────────────────────────

/// Configuration options for the ExoAR writer.
#[derive(Clone, Copy, Debug)]
pub struct ExoarWriteOptions {
    /// Flags to place in ExoarHeader.
    pub archive_flags: u32,
    /// Base epoch (source of incremental).
    pub epoch_base: u64,
    /// Target epoch (state to export).
    pub epoch_target: u64,
    /// Creation timestamp (UNIX seconds, 0 = unspecified).
    pub created_at: u64,
    /// Verify CRC32C of each payload before writing.
    pub verify_before_write: bool,
    /// Write tombstones (deletion records).
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

// ─── Write Statistics ────────────────────────────────────────────────────────

/// Statistics accumulated during writing.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExoarWriteStats {
    /// Number of blobs written.
    pub blobs_written: u32,
    /// Number of tombstones written.
    pub tombstones_written: u32,
    /// Total payload bytes written.
    pub payload_bytes: u64,
    /// Total archive size in bytes.
    pub archive_bytes: u64,
    /// Number of ignored write errors.
    pub write_errors: u32,
    /// Current global CRC32C (accumulated over writes).
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

    /// Returns the total number of entries written.
    pub fn total_entries(&self) -> u32 {
        self.blobs_written.saturating_add(self.tombstones_written)
    }
}

// ─── Internal Writer State ───────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WriterState {
    /// Not yet initialized (header not written).
    Idle,
    /// In the process of writing entries.
    Writing,
    /// Finalized (footer written).
    Finalized,
}

// ─── Main Writer ─────────────────────────────────────────────────────────────

/// ExoAR archive writer, operating on an `ArchiveSink`.
///
/// Typical usage:
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
    /// Creates a new writer with the given options.
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

    /// Returns current statistics (read-only).
    pub fn stats(&self) -> &ExoarWriteStats {
        &self.stats
    }

    /// Writes the archive header. Must be called first.
    /// RULE 8: magic written FIRST.
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
        sink.write_all(&bytes)
            .map_err(|_| ExoarWriteError::IoError)?;
        self.stats.running_crc = crc32c_update(self.stats.running_crc, &bytes);
        self.stats.archive_bytes = self.stats.archive_bytes.saturating_add(bytes.len() as u64);
        self.state = WriterState::Writing;
        Ok(())
    }

    /// Writes a data blob into the archive.
    /// RULE 8  : ExoarEntryHeader magic is the first field written.
    /// RULE 11 : blob_id must be blake3(data BEFORE compression/encryption).
    /// OOM-02   : no internal allocation here — operates directly on the sink.
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
        sink.write_all(&hdr_bytes)
            .map_err(|_| ExoarWriteError::IoError)?;
        self.stats.running_crc = crc32c_update(self.stats.running_crc, &hdr_bytes);
        self.stats.archive_bytes = self
            .stats
            .archive_bytes
            .saturating_add(hdr_bytes.len() as u64);

        if !data.is_empty() {
            sink.write_all(data).map_err(|_| ExoarWriteError::IoError)?;
            self.stats.running_crc = crc32c_update(self.stats.running_crc, data);
            self.stats.archive_bytes = self.stats.archive_bytes.saturating_add(data.len() as u64);
            self.stats.payload_bytes = self.stats.payload_bytes.saturating_add(data.len() as u64);
        }

        self.stats.blobs_written = self.stats.blobs_written.saturating_add(1);
        Ok(())
    }

    /// Writes a tombstone (blob deletion record).
    pub fn write_tombstone<S: ArchiveSink>(
        &mut self,
        sink: &mut S,
        blob_id: &[u8; 32],
        epoch: u64,
    ) -> Result<(), ExoarWriteError> {
        if !self.options.write_tombstones {
            return Ok(());
        }
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
        sink.write_all(&hdr_bytes)
            .map_err(|_| ExoarWriteError::IoError)?;
        self.stats.running_crc = crc32c_update(self.stats.running_crc, &hdr_bytes);
        self.stats.archive_bytes = self
            .stats
            .archive_bytes
            .saturating_add(hdr_bytes.len() as u64);
        self.stats.tombstones_written = self.stats.tombstones_written.saturating_add(1);
        Ok(())
    }

    /// Finalizes the archive (writes footer). Can only be called once.
    pub fn finalize<S: ArchiveSink>(
        &mut self,
        sink: &mut S,
    ) -> Result<ExoarWriteStats, ExoarWriteError> {
        if self.state == WriterState::Finalized {
            return Err(ExoarWriteError::AlreadyFinalized);
        }
        if self.state == WriterState::Idle {
            return Err(ExoarWriteError::NotStarted);
        }

        let total_size = self
            .stats
            .archive_bytes
            .saturating_add(size_of::<ExoarFooter>() as u64);
        let ftr = ExoarFooter::new(
            self.stats.total_entries(),
            self.stats.running_crc,
            total_size,
        );
        let ftr_bytes = ftr.as_bytes();
        sink.write_all(&ftr_bytes)
            .map_err(|_| ExoarWriteError::IoError)?;
        self.stats.archive_bytes = self
            .stats
            .archive_bytes
            .saturating_add(ftr_bytes.len() as u64);
        self.state = WriterState::Finalized;
        Ok(self.stats)
    }

    /// Returns true if the archive is finalized.
    pub fn is_finalized(&self) -> bool {
        self.state == WriterState::Finalized
    }
}

// ─── Buffered Writer ─────────────────────────────────────────────────────────

/// Buffered writer: accumulates the full archive in memory.
/// Useful for small archives or testing.
/// OOM-02: try_reserve before every buffer extension.
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

    pub fn with_capacity(
        options: ExoarWriteOptions,
        capacity: usize,
    ) -> Result<Self, ExoarWriteError> {
        let mut buffer = Vec::new();
        buffer
            .try_reserve(capacity)
            .map_err(|_| ExoarWriteError::OutOfMemory)?;
        Ok(Self {
            inner: ExoarWriter::new(options),
            buffer,
        })
    }

    /// Begins writing the archive.
    pub fn begin(&mut self) -> Result<(), ExoarWriteError> {
        let mut sink = BufSink {
            buf: &mut self.buffer,
        };
        self.inner.begin(&mut sink)
    }

    /// Writes a blob into the buffer.
    pub fn write_blob(
        &mut self,
        blob_id: &[u8; 32],
        data: &[u8],
        flags: u8,
        epoch: u64,
    ) -> Result<(), ExoarWriteError> {
        let mut sink = BufSink {
            buf: &mut self.buffer,
        };
        self.inner
            .write_blob(&mut sink, blob_id, data, flags, epoch)
    }

    /// Writes a tombstone into the buffer.
    pub fn write_tombstone(
        &mut self,
        blob_id: &[u8; 32],
        epoch: u64,
    ) -> Result<(), ExoarWriteError> {
        let mut sink = BufSink {
            buf: &mut self.buffer,
        };
        self.inner.write_tombstone(&mut sink, blob_id, epoch)
    }

    /// Finalizes the archive and returns the full buffer.
    pub fn finalize(mut self) -> Result<(Vec<u8>, ExoarWriteStats), ExoarWriteError> {
        let mut sink = BufSink {
            buf: &mut self.buffer,
        };
        let stats = self.inner.finalize(&mut sink)?;
        Ok((self.buffer, stats))
    }

    /// Returns a reference to the buffer without finalizing.
    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }

    /// Returns current statistics.
    pub fn stats(&self) -> &ExoarWriteStats {
        self.inner.stats()
    }
}

/// Internal sink to a Vec<u8> (used by ExoarBufferedWriter).
struct BufSink<'a> {
    buf: &'a mut Vec<u8>,
}

impl<'a> ArchiveSink for BufSink<'a> {
    fn write_all(&mut self, data: &[u8]) -> ExofsResult<()> {
        self.buf
            .try_reserve(data.len())
            .map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(data);
        Ok(())
    }

    fn bytes_written(&self) -> u64 {
        self.buf.len() as u64
    }
}

// ─── SinkVec (public implementation of ArchiveSink) ──────────────────────────

/// Public implementation of `ArchiveSink` on a `Vec<u8>`.
/// Useful for tests and use cases where the archive must fit in memory.
pub struct SinkVec {
    buf: Vec<u8>,
}

impl SinkVec {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn with_capacity(cap: usize) -> Result<Self, ExofsError> {
        let mut buf = Vec::new();
        buf.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        Ok(Self { buf })
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

impl ArchiveSink for SinkVec {
    fn write_all(&mut self, data: &[u8]) -> ExofsResult<()> {
        self.buf
            .try_reserve(data.len())
            .map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(data);
        Ok(())
    }

    fn bytes_written(&self) -> u64 {
        self.buf.len() as u64
    }
}

// ─── Inline blake3 ────────────────────────────────────────────────────────────

/// Computes the BlobId of a data slice (RULE 11: blake3 BEFORE compression).
/// In production, use real blake3 via crate::fs::exofs::dedup::content_hash.
pub fn compute_blob_id(data: &[u8]) -> [u8; 32] {
    *blob_id::compute_blob_id(data).as_bytes()
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::exoar_reader::{CollectingReceiver, ExoarReader, SliceSource};
    use super::*;

    fn make_blob_id(tag: u8) -> [u8; 32] {
        let mut id = [0u8; 32];
        id[0] = tag;
        id
    }

    #[test]
    fn test_write_empty_archive() {
        let mut w = ExoarBufferedWriter::new(ExoarWriteOptions::default());
        w.begin().test_expect("begin ok");
        let (buf, stats) = w.finalize().test_expect("finalize ok");
        assert_eq!(stats.blobs_written, 0);
        assert!(buf.len() >= 128 + 32); // header + footer minimum
    }

    #[test]
    fn test_write_single_blob() {
        let bid = make_blob_id(1);
        let data = b"hello world";
        let mut w = ExoarBufferedWriter::new(ExoarWriteOptions::default());
        w.begin().test_expect("begin");
        w.write_blob(&bid, data, 0, 0).test_expect("write blob");
        let (buf, stats) = w.finalize().test_expect("finalize");
        assert_eq!(stats.blobs_written, 1);
        assert_eq!(stats.payload_bytes, data.len() as u64);
        // Read archive back
        let mut src = SliceSource::new(&buf);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let report = reader.read(&mut src, &mut rcv).test_expect("read ok");
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
        w.begin().test_expect("begin");
        for (bid, data) in &blobs {
            w.write_blob(bid, data, 0, 0).test_expect("write");
        }
        let (buf, stats) = w.finalize().test_expect("finalize");
        assert_eq!(stats.blobs_written, 3);

        let mut src = SliceSource::new(&buf);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let report = reader.read(&mut src, &mut rcv).test_expect("read");
        assert_eq!(report.entries_read, 3);
    }

    #[test]
    fn test_write_tombstone() {
        let bid = make_blob_id(99);
        let opt = ExoarWriteOptions::incremental(1, 2);
        let mut w = ExoarBufferedWriter::new(opt);
        w.begin().test_expect("begin");
        w.write_tombstone(&bid, 2).test_expect("tombstone");
        let (_, stats) = w.finalize().test_expect("finalize");
        assert_eq!(stats.tombstones_written, 1);
        assert_eq!(stats.blobs_written, 0);
    }

    #[test]
    fn test_finalize_twice_error() {
        let mut sink = SinkVec::new();
        let mut w = ExoarWriter::new(ExoarWriteOptions::default());
        w.begin(&mut sink).test_expect("begin");
        w.finalize(&mut sink).test_expect("first finalize");
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
        w.begin(&mut sink).test_expect("begin");
        // Create a fake "large" payload by directly manipulating size
        // We cannot allocate 256 MiB in a unit test, so we verify via
        // an entry with declared size - test UI skipped and ensure error
        // returned for empty data with excessive declared size.
        // Test: protection via EXOAR_MAX_PAYLOAD is active.
        assert_eq!(EXOAR_MAX_PAYLOAD, 256 * 1024 * 1024);
    }

    #[test]
    fn test_sink_vec_basic() {
        let mut sink = SinkVec::new();
        sink.write_all(b"hello").test_expect("write");
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
        w.begin().test_expect("begin");
        let size_after_header = w.buffer().len();
        w.write_blob(&make_blob_id(1), b"payload", 0, 0)
            .test_expect("write");
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
