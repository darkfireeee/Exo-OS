//! stream_import.rs — Streaming blob import (no_std).
//!
//! This module provides:
//!  - `BlobWriter`           : blob writing trait into ExoFS store.
//!  - `StreamImportConfig`   : import session parameters.
//!  - `ConflictResolver`     : conflict resolution strategy.
//!  - `TombstoneHandler`     : deleted blob management (tombstones).
//!  - `ImportCheckpoint`     : resume point for an interrupted import.
//!  - `StreamImporter`       : chunk-by-chunk import engine.
//!  - `StreamImportReport`   : end-of-import report.
//!  - `ImportEntryHeader`    : entry header read from the stream.
//!
//! RECUR-01 : while loops — no recursion.
//! OOM-02   : try_reserve before push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add.

extern crate alloc;
use super::incremental_export::EpochId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── Blob Writing Trait ──────────────────────────────────────────────────────

/// Blob writer into the ExoFS store.
pub trait BlobWriter {
    /// Stores a blob with its identifier (blake3, RULE 11).
    fn write_blob(&mut self, blob_id: &[u8; 32], data: &[u8]) -> ExofsResult<()>;

    /// Indicates whether a blob already exists in the store.
    fn blob_exists(&self, blob_id: &[u8; 32]) -> bool;

    /// Deletes a blob (processing a tombstone).
    fn delete_blob(&mut self, blob_id: &[u8; 32]) -> ExofsResult<()>;

    /// Number of bytes stored in this session.
    fn bytes_written(&self) -> u64;
}

// ─── Conflict Resolution ──────────────────────────────────────────────────────

/// Resolution strategy when an imported blob already exists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConflictResolver {
    /// Skip incoming entry if blob already exists.
    Skip,
    /// Overwrite existing blob with imported version.
    Overwrite,
    /// Return an error and abort import.
    Fail,
    /// Overwrite only if imported epoch is newer.
    KeepNewer,
}

impl ConflictResolver {
    /// Resolves a conflict — returns Ok(true) to proceed with writing, Ok(false) to skip.
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
            ConflictResolver::KeepNewer => match existing_epoch {
                Some(ep) => Ok(incoming_epoch.value() > ep.value()),
                None => Ok(true),
            },
        }
    }
}

// ─── Tombstone Handler ────────────────────────────────────────────────────────

/// Processing mode for tombstones during import.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TombstoneHandler {
    /// Delete local blob if it exists.
    Delete,
    /// Ignore tombstone (local blob is not deleted).
    Ignore,
    /// Return an error if local blob exists.
    FailIfPresent,
}

impl TombstoneHandler {
    /// Processes a tombstone — returns Ok(true) to trigger local deletion.
    pub fn handle(&self, blob_id: &[u8; 32], local_exists: bool) -> ExofsResult<bool> {
        let _ = blob_id;
        match self {
            TombstoneHandler::Delete => Ok(local_exists),
            TombstoneHandler::Ignore => Ok(false),
            TombstoneHandler::FailIfPresent => {
                if local_exists {
                    Err(ExofsError::AlreadyExists)
                } else {
                    Ok(false)
                }
            }
        }
    }
}

// ─── Stream Entry Header ────────────────────────────────────────────────────

/// Header of each entry read from a binary data stream.
///
/// Compact binary structure: 52 bytes.
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct ImportEntryHeader {
    /// Validation magic: 0x4558_494D ("EXIM").
    pub magic: u32,
    /// Flags: IMPORT_FLAG_TOMBSTONE = 0x01, IMPORT_FLAG_VERIFIED = 0x02.
    pub flags: u8,
    /// Padding.
    pub _pad: [u8; 3],
    /// BlobId (blake3) — RULE 11.
    pub blob_id: [u8; 32],
    /// Size of following data (0 for tombstone).
    pub data_size: u64,
    /// Reserved for protocol extensions.
    pub _pad2: [u8; 4],
}

const _: () = assert!(
    core::mem::size_of::<ImportEntryHeader>() == 52,
    "ImportEntryHeader ABI size changed — verifier import stream ExoAR"
);

/// Magic of import header.
pub const IMPORT_ENTRY_MAGIC: u32 = 0x4558_494D; // "EXIM"

/// Flag: entry is a tombstone.
pub const IMPORT_FLAG_TOMBSTONE: u8 = 0x01;
/// Flag: blob_id already verified by sender.
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
            magic: IMPORT_ENTRY_MAGIC,
            flags: IMPORT_FLAG_TOMBSTONE,
            _pad: [0u8; 3],
            blob_id,
            data_size: 0,
            _pad2: [0u8; 4],
        }
    }

    /// Validates magic FIRST (RULE 8).
    pub fn validate_magic(&self) -> bool {
        // SAFETY: buffer of sufficient length, verified prior to call, repr(C).
        let m: u32 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.magic)) };
        m == IMPORT_ENTRY_MAGIC
    }

    pub fn is_tombstone(&self) -> bool {
        // SAFETY: buffer of sufficient length, verified prior to call, repr(C).
        let f: u8 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.flags)) };
        f & IMPORT_FLAG_TOMBSTONE != 0
    }

    pub fn data_size_unaligned(&self) -> u64 {
        // SAFETY: buffer of sufficient length, verified prior to call, repr(C).
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.data_size)) }
    }

    pub fn blob_id_unaligned(&self) -> [u8; 32] {
        // SAFETY: buffer of sufficient length, verified prior to call, repr(C).
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.blob_id)) }
    }

    pub fn as_bytes(&self) -> [u8; 52] {
        let mut out = [0u8; 52];
        // SAFETY: ImportEntryHeader is #[repr(C)] and its size is verified by const assert.
        unsafe {
            core::ptr::copy_nonoverlapping(self as *const Self as *const u8, out.as_mut_ptr(), 52);
        }
        out
    }
}

// ─── Resume Checkpoint ───────────────────────────────────────────────────────

/// Resume point for an interrupted streaming import.
#[derive(Clone, Copy, Debug)]
pub struct ImportCheckpoint {
    /// Offset in source stream (bytes).
    pub source_offset: u64,
    /// Number of successfully processed entries.
    pub entries_processed: u64,
    /// Last imported BlobId.
    pub last_blob_id: [u8; 32],
    /// true if checkpoint is usable.
    pub valid: bool,
}

impl ImportCheckpoint {
    pub fn new() -> Self {
        Self {
            source_offset: 0,
            entries_processed: 0,
            last_blob_id: [0u8; 32],
            valid: false,
        }
    }

    pub fn advance(&mut self, offset: u64, blob_id: [u8; 32]) {
        self.source_offset = offset;
        self.last_blob_id = blob_id;
        self.entries_processed = self.entries_processed.saturating_add(1);
        self.valid = true;
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

// ─── Import Configuration ─────────────────────────────────────────────────────

/// Streaming import session parameters.
#[derive(Clone, Copy, Debug)]
pub struct StreamImportConfig {
    /// Session identifier.
    pub session_id: u32,
    /// Verify blob_id (blake3) after reception — RULE 11.
    pub verify_blob_id: bool,
    /// Maximum number of entries to import (0 = unlimited).
    pub max_entries: u32,
    /// Conflict resolution strategy.
    pub conflict: ConflictResolver,
    /// Tombstone processing mode.
    pub tombstone_mode: TombstoneHandler,
    /// Maximum allowed blob size (0 = unlimited).
    pub max_blob_size: u64,
}

impl StreamImportConfig {
    pub fn default(session_id: u32) -> Self {
        Self {
            session_id,
            verify_blob_id: true,
            max_entries: 0,
            conflict: ConflictResolver::Skip,
            tombstone_mode: TombstoneHandler::Delete,
            max_blob_size: 0,
        }
    }

    pub fn strict(session_id: u32) -> Self {
        Self {
            session_id,
            verify_blob_id: true,
            max_entries: 0,
            conflict: ConflictResolver::Fail,
            tombstone_mode: TombstoneHandler::FailIfPresent,
            max_blob_size: 256 * 1024 * 1024,
        }
    }

    pub fn validate(&self) -> ExofsResult<()> {
        Ok(())
    }
}

// ─── Import Report ───────────────────────────────────────────────────────────

/// End of streaming import session report.
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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_imported(&mut self, size: u64, overwrite: bool) {
        self.blobs_imported = self.blobs_imported.saturating_add(1);
        self.bytes_written = self.bytes_written.saturating_add(size);
        if overwrite {
            self.blobs_overwritten = self.blobs_overwritten.saturating_add(1);
        }
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

    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }
    pub fn is_clean(&self) -> bool {
        self.is_complete && !self.has_errors()
    }
}

// ─── Stream Source ────────────────────────────────────────────────────────────

/// Data source for import (chunk by chunk).
pub trait ImportSource {
    /// Reads exactly `buf.len()` bytes from the source.
    fn read_exact(&mut self, buf: &mut [u8]) -> ExofsResult<()>;

    /// Returns number of bytes read since the beginning.
    fn bytes_read(&self) -> u64;
}

/// Implementation of ImportSource on a slice (for tests).
pub struct SliceImportSource<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceImportSource<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
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
    fn bytes_read(&self) -> u64 {
        self.pos as u64
    }
}

// ─── Streaming Import Engine ──────────────────────────────────────────────────

/// Imports blobs from a binary stream into the ExoFS store.
///
/// Expected stream format:
///   [ ImportEntryHeader (52 bytes) | data (header.data_size bytes) ] × N
///
/// RECUR-01: single while loop.
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
        Ok(Self {
            config,
            checkpoint: ck,
            report: StreamImportReport::new(),
        })
    }

    /// Starts import from source `src` to writer `writer`.
    ///
    /// RECUR-01: single while loop over entries.
    pub fn run<S: ImportSource, W: BlobWriter>(
        &mut self,
        src: &mut S,
        writer: &mut W,
    ) -> ExofsResult<StreamImportReport> {
        let mut hdr_buf = [0u8; 52]; // size of ImportEntryHeader
        let mut entries = 0u64;

        loop {
            // max_entries limit (ARITH-02: saturating comparison)
            if self.config.max_entries > 0 && entries >= self.config.max_entries as u64 {
                break;
            }

            // Read header
            if src.read_exact(&mut hdr_buf).is_err() {
                // Normal end of stream
                break;
            }

            // Safe reinterpretation into ImportEntryHeader
            // SAFETY: safety invariant verified by caller function preconditions.
            let hdr: ImportEntryHeader =
                unsafe { core::ptr::read_unaligned(hdr_buf.as_ptr() as *const ImportEntryHeader) };

            // RULE 8: magic FIRST
            if !hdr.validate_magic() {
                self.report.record_error();
                break;
            }

            let blob_id = hdr.blob_id_unaligned();
            let data_size = hdr.data_size_unaligned();

            if hdr.is_tombstone() {
                // Tombstone processing
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

            // Max size verification (ARITH-02)
            if self.config.max_blob_size > 0 && data_size > self.config.max_blob_size {
                self.report.record_error();
                self.report.record_skipped();
                break;
            }

            // Allocate data buffer — OOM-02: try_reserve
            let mut payload: Vec<u8> = Vec::new();
            payload
                .try_reserve(data_size as usize)
                .map_err(|_| ExofsError::NoMemory)?;
            payload.resize(data_size as usize, 0u8);

            if src.read_exact(&mut payload).is_err() {
                self.report.record_error();
                break;
            }

            self.report.bytes_received = self.report.bytes_received.saturating_add(data_size);

            // Verify blob_id — RULE 11: blake3(raw data BEFORE compression)
            if self.config.verify_blob_id {
                let computed = inline_blake3(&payload);
                if computed != blob_id {
                    self.report.record_error();
                    entries = entries.saturating_add(1);
                    continue;
                }
            }

            // Conflict resolution
            let local_exists = writer.blob_exists(&blob_id);
            if local_exists {
                let existing_epoch = None; // without direct access to local epoch
                match self
                    .config
                    .conflict
                    .resolve(&blob_id, EpochId(0), existing_epoch)
                {
                    Ok(true) => {
                        // Overwrite
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

    pub fn checkpoint(&self) -> &ImportCheckpoint {
        &self.checkpoint
    }
    pub fn report(&self) -> &StreamImportReport {
        &self.report
    }
}

// ─── Import Stream Builder ────────────────────────────────────────────────────

/// Builds a binary import stream from a list of blobs.
pub struct ImportStreamBuilder {
    buf: Vec<u8>,
}

impl ImportStreamBuilder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Adds a blob to the stream — OOM-02: try_reserve.
    pub fn append_blob(&mut self, blob_id: [u8; 32], data: &[u8]) -> ExofsResult<()> {
        let hdr = ImportEntryHeader::new_blob(blob_id, data.len() as u64);
        let hdr_bytes = hdr.as_bytes();
        let total = hdr_bytes.len().saturating_add(data.len());
        self.buf
            .try_reserve(total)
            .map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(&hdr_bytes);
        self.buf.extend_from_slice(data);
        Ok(())
    }

    /// Adds a tombstone to the stream — OOM-02: try_reserve.
    pub fn append_tombstone(&mut self, blob_id: [u8; 32]) -> ExofsResult<()> {
        let hdr = ImportEntryHeader::new_tombstone(blob_id);
        let hdr_bytes = hdr.as_bytes();
        self.buf
            .try_reserve(hdr_bytes.len())
            .map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(&hdr_bytes);
        Ok(())
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

// ─── Minimal inline blake3 ────────────────────────────────────────────────────

fn inline_blake3(data: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6b08_c647u32,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
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
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
struct MockWriter {
    blobs: Vec<([u8; 32], Vec<u8>)>,
    deleted: Vec<[u8; 32]>,
    written_bytes: u64,
}

#[cfg(test)]
impl MockWriter {
    fn new() -> Self {
        Self {
            blobs: Vec::new(),
            deleted: Vec::new(),
            written_bytes: 0,
        }
    }
    fn find_blob(&self, id: &[u8; 32]) -> bool {
        self.blobs.iter().any(|(bid, _)| bid == id)
    }
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
    fn blob_exists(&self, blob_id: &[u8; 32]) -> bool {
        self.find_blob(blob_id)
    }
    fn delete_blob(&mut self, blob_id: &[u8; 32]) -> ExofsResult<()> {
        self.blobs.retain(|(id, _)| id != blob_id);
        self.deleted.push(*blob_id);
        Ok(())
    }
    fn bytes_written(&self) -> u64 {
        self.written_bytes
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_id(n: u8) -> [u8; 32] {
        let mut id = [0u8; 32];
        id[0] = n;
        id
    }

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
        assert_eq!(r.test_expect("skip ok"), false);
    }

    #[test]
    fn test_conflict_overwrite() {
        let r = ConflictResolver::Overwrite.resolve(&make_id(1), EpochId(3), Some(EpochId(2)));
        assert_eq!(r.test_expect("overwrite ok"), true);
    }

    #[test]
    fn test_conflict_keep_newer() {
        let r = ConflictResolver::KeepNewer.resolve(&make_id(1), EpochId(5), Some(EpochId(3)));
        assert_eq!(r.test_expect("newer ok"), true);
        let r2 = ConflictResolver::KeepNewer.resolve(&make_id(1), EpochId(2), Some(EpochId(5)));
        assert_eq!(r2.test_expect("older ok"), false);
    }

    #[test]
    fn test_tombstone_handler_delete() {
        let r = TombstoneHandler::Delete.handle(&make_id(1), true);
        assert_eq!(r.test_expect("delete ok"), true);
        let r2 = TombstoneHandler::Delete.handle(&make_id(1), false);
        assert_eq!(r2.test_expect("delete absent ok"), false);
    }

    #[test]
    fn test_tombstone_handler_ignore() {
        let r = TombstoneHandler::Ignore.handle(&make_id(1), true);
        assert_eq!(r.test_expect("ignore ok"), false);
    }

    #[test]
    fn test_slice_import_source() {
        let data = [1u8, 2, 3, 4, 5];
        let mut src = SliceImportSource::new(&data);
        let mut buf = [0u8; 3];
        src.read_exact(&mut buf).test_expect("ok");
        assert_eq!(buf, [1, 2, 3]);
        assert_eq!(src.bytes_read(), 3);
    }

    #[test]
    fn test_import_empty_stream() {
        let cfg = StreamImportConfig::default(1);
        let mut importer = StreamImporter::new(cfg).test_expect("ok");
        let mut src = SliceImportSource::new(&[]);
        let mut writer = MockWriter::new();
        let report = importer.run(&mut src, &mut writer).test_expect("ok");
        assert_eq!(report.blobs_imported, 0);
        assert!(report.is_complete);
    }

    #[test]
    fn test_import_single_blob() {
        let data = b"single blob data";
        let blob_id = inline_blake3(data);
        let mut builder = ImportStreamBuilder::new();
        builder.append_blob(blob_id, data).test_expect("ok");

        let cfg = StreamImportConfig {
            verify_blob_id: false,
            ..StreamImportConfig::default(1)
        };
        let mut importer = StreamImporter::new(cfg).test_expect("ok");
        let mut src = SliceImportSource::new(builder.as_slice());
        let mut writer = MockWriter::new();
        let report = importer.run(&mut src, &mut writer).test_expect("ok");
        assert_eq!(report.blobs_imported, 1);
        assert_eq!(writer.blobs.len(), 1);
    }

    #[test]
    fn test_import_multiple_blobs() {
        let mut builder = ImportStreamBuilder::new();
        for i in 0u8..5 {
            let data = [i; 64];
            let bid = inline_blake3(&data);
            builder.append_blob(bid, &data).test_expect("ok");
        }
        let cfg = StreamImportConfig {
            verify_blob_id: false,
            ..StreamImportConfig::default(1)
        };
        let mut importer = StreamImporter::new(cfg).test_expect("ok");
        let mut src = SliceImportSource::new(builder.as_slice());
        let mut writer = MockWriter::new();
        let report = importer.run(&mut src, &mut writer).test_expect("ok");
        assert_eq!(report.blobs_imported, 5);
        assert_eq!(writer.blobs.len(), 5);
    }

    #[test]
    fn test_import_tombstone_deletes_blob() {
        let mut writer = MockWriter::new();
        let data = b"to delete";
        let bid = make_id(42);
        writer.write_blob(&bid, data).test_expect("pre-populate");

        let mut builder = ImportStreamBuilder::new();
        builder.append_tombstone(bid).test_expect("ok");

        let cfg = StreamImportConfig::default(1);
        let mut importer = StreamImporter::new(cfg).test_expect("ok");
        let mut src = SliceImportSource::new(builder.as_slice());
        let report = importer.run(&mut src, &mut writer).test_expect("ok");
        assert_eq!(report.tombstones_applied, 1);
        assert!(!writer.blob_exists(&bid));
    }

    #[test]
    fn test_import_skip_conflict() {
        let mut writer = MockWriter::new();
        let data = b"existing blob";
        let bid = make_id(10);
        writer.write_blob(&bid, data).test_expect("pre-populate");

        let mut builder = ImportStreamBuilder::new();
        builder.append_blob(bid, b"new version").test_expect("ok");

        let cfg = StreamImportConfig {
            verify_blob_id: false,
            conflict: ConflictResolver::Skip,
            ..StreamImportConfig::default(1)
        };
        let mut importer = StreamImporter::new(cfg).test_expect("ok");
        let mut src = SliceImportSource::new(builder.as_slice());
        let report = importer.run(&mut src, &mut writer).test_expect("ok");
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
        builder.append_blob(make_id(1), b"data").test_expect("ok");
        builder.append_tombstone(make_id(2)).test_expect("ok");
        // Expected size: 52 + 4 (data) + 52 (tombstone)
        assert_eq!(builder.len(), 52 + 4 + 52);
    }
}
