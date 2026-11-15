//! stream_export.rs — Streaming blob export (no_std, no full archive in RAM).
//!
//! This module provides:
//!  - `BlobDataProvider`    : blob data provider trait.
//!  - `StreamSink`          : chunk-by-chunk output trait.
//!  - `StreamExportConfig`  : streaming export parameters.
//!  - `StreamFilter`        : selective filter on exported blobs.
//!  - `StreamExporter`      : export engine without full archive in RAM.
//!  - `StreamExportBatch`   : export batch with checkpoint.
//!  - `StreamExportReport`  : end-of-export report.
//!
//! RECUR-01 : while loops — no recursion.
//! OOM-02   : try_reserve before push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add.

extern crate alloc;
use super::incremental_export::EpochId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── Data Provider Trait ──────────────────────────────────────────────────

/// Blob data provider — implemented by the ExoFS storage layer.
pub trait BlobDataProvider {
    /// Reads data of a blob identified by its BlobId (blake3 RULE 11).
    /// Returns a slice valid until the next call.
    fn provide_blob(&self, blob_id: &[u8; 32]) -> ExofsResult<&[u8]>;

    /// Returns the creation epoch of a blob.
    fn blob_epoch(&self, blob_id: &[u8; 32]) -> ExofsResult<EpochId>;

    /// Indicates whether a blob exists in the store.
    fn blob_exists(&self, blob_id: &[u8; 32]) -> bool;
}

// ─── Streaming Output Trait ────────────────────────────────────────────────

/// Export chunk receiver (streaming).
pub trait StreamSink {
    /// Receives a raw data chunk.
    fn write_chunk(&mut self, data: &[u8]) -> ExofsResult<()>;

    /// Indicates the start of a new blob entry.
    fn begin_entry(&mut self, blob_id: &[u8; 32], size: u64) -> ExofsResult<()>;

    /// Finalizes the current entry.
    fn end_entry(&mut self) -> ExofsResult<()>;

    /// Number of bytes written to the sink.
    fn bytes_written(&self) -> u64;
}

// ─── Export Filter ──────────────────────────────────────────────────────────

/// Filtering mode applied to blobs during export.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FilterMode {
    /// Export all blobs (no filter).
    AcceptAll,
    /// Only export blobs whose epoch is in [min, max].
    EpochRange { min: u64, max: u64 },
    /// Only export blobs with size < max_size.
    MaxSize(u64),
    /// Exclude blobs whose size > threshold (large blobs ignored).
    ExcludeLarge(u64),
}

/// Filter for selecting blobs during export.
pub struct StreamFilter {
    mode: FilterMode,
    /// Number of accepted blobs.
    accepted: u64,
    /// Number of rejected blobs.
    rejected: u64,
}

impl StreamFilter {
    pub fn new(mode: FilterMode) -> Self {
        Self {
            mode,
            accepted: 0,
            rejected: 0,
        }
    }

    pub fn accept_all() -> Self {
        Self::new(FilterMode::AcceptAll)
    }

    pub fn epoch_range(min: u64, max: u64) -> Self {
        Self::new(FilterMode::EpochRange { min, max })
    }

    /// Tests whether a blob should be exported.
    pub fn should_export(&mut self, blob_id: &[u8; 32], epoch: EpochId, size: u64) -> bool {
        let _ = blob_id;
        let accept = match self.mode {
            FilterMode::AcceptAll => true,
            FilterMode::EpochRange { min, max } => epoch.value() >= min && epoch.value() <= max,
            FilterMode::MaxSize(limit) => size < limit,
            FilterMode::ExcludeLarge(threshold) => size <= threshold,
        };
        if accept {
            self.accepted = self.accepted.saturating_add(1);
        } else {
            self.rejected = self.rejected.saturating_add(1);
        }
        accept
    }

    pub fn accepted(&self) -> u64 {
        self.accepted
    }
    pub fn rejected(&self) -> u64 {
        self.rejected
    }
}

// ─── Streaming Export Configuration ────────────────────────────────────────

/// Streaming export session parameters.
#[derive(Clone, Copy, Debug)]
pub struct StreamExportConfig {
    /// Session identifier.
    pub session_id: u32,
    /// Verify blob_id (RULE 11) before export.
    pub verify_blob_id: bool,
    /// Transfer block size (must divide payload).
    pub block_size: u32,
    /// Maximum number of blobs to export (0 = unlimited).
    pub max_blobs: u32,
    /// Emit tombstones for deleted blobs.
    pub include_tombstones: bool,
    /// Minimum epoch (0 = all from beginning).
    pub epoch_min: u64,
    /// Maximum epoch (u64::MAX = up to current epoch).
    pub epoch_max: u64,
}

impl StreamExportConfig {
    pub fn default(session_id: u32) -> Self {
        Self {
            session_id,
            verify_blob_id: true,
            block_size: 65536,
            max_blobs: 0,
            include_tombstones: true,
            epoch_min: 0,
            epoch_max: u64::MAX,
        }
    }

    pub fn minimal(session_id: u32) -> Self {
        Self {
            session_id,
            verify_blob_id: false,
            block_size: 4096,
            max_blobs: 0,
            include_tombstones: false,
            epoch_min: 0,
            epoch_max: u64::MAX,
        }
    }

    /// Validates configuration — returns Err if invalid.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.block_size == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if self.epoch_min > self.epoch_max {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }
}

// ─── Streaming Export Report ───────────────────────────────────────────────

/// End of streaming export session report.
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamExportReport {
    pub blobs_exported: u64,
    pub blobs_skipped: u64,
    pub tombstones_emitted: u64,
    pub bytes_exported: u64,
    pub bytes_written_to_sink: u64,
    pub errors: u32,
    pub is_complete: bool,
    /// Last exported blob_id (for checkpoint).
    pub last_blob_id: [u8; 32],
}

impl StreamExportReport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_exported(&mut self, size: u64) {
        self.blobs_exported = self.blobs_exported.saturating_add(1);
        self.bytes_exported = self.bytes_exported.saturating_add(size);
    }

    pub fn record_skipped(&mut self) {
        self.blobs_skipped = self.blobs_skipped.saturating_add(1);
    }

    pub fn record_tombstone(&mut self) {
        self.tombstones_emitted = self.tombstones_emitted.saturating_add(1);
    }

    pub fn record_error(&mut self) {
        self.errors = self.errors.saturating_add(1);
    }

    pub fn total_entries(&self) -> u64 {
        self.blobs_exported.saturating_add(self.tombstones_emitted)
    }

    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }
    pub fn is_clean(&self) -> bool {
        self.is_complete && !self.has_errors()
    }
}

// ─── Streaming Export Checkpoint ───────────────────────────────────────────

/// Resume point for an interrupted streaming export.
#[derive(Clone, Copy, Debug)]
pub struct StreamCheckpoint {
    /// Index of next blob to export in the list.
    pub next_index: usize,
    /// BlobId of last successfully exported blob.
    pub last_exported: [u8; 32],
    /// Number of blobs exported up to this point.
    pub exported_so_far: u64,
    /// Epoch of last exported blob.
    pub last_epoch: u64,
    /// true if checkpoint can be used for resume.
    pub valid: bool,
}

impl StreamCheckpoint {
    pub fn new() -> Self {
        Self {
            next_index: 0,
            last_exported: [0u8; 32],
            exported_so_far: 0,
            last_epoch: 0,
            valid: false,
        }
    }

    pub fn advance(&mut self, blob_id: [u8; 32], epoch: u64) {
        self.next_index = self.next_index.wrapping_add(1);
        self.last_exported = blob_id;
        self.last_epoch = epoch;
        self.exported_so_far = self.exported_so_far.saturating_add(1);
        self.valid = true;
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

// ─── Streaming Export Engine ────────────────────────────────────────────────

/// Exports blobs to a StreamSink chunk by chunk, without full archive in RAM.
///
/// Export is performed in two passes:
///   1. Active blobs (RECUR-01: while loop).
///   2. Tombstones (RECUR-01: while loop).
pub struct StreamExporter {
    config: StreamExportConfig,
    checkpoint: StreamCheckpoint,
    report: StreamExportReport,
}

impl StreamExporter {
    pub fn new(config: StreamExportConfig) -> ExofsResult<Self> {
        config.validate()?;
        Ok(Self {
            config,
            checkpoint: StreamCheckpoint::new(),
            report: StreamExportReport::new(),
        })
    }

    /// Resumes an export from a previous checkpoint.
    pub fn resume(config: StreamExportConfig, checkpoint: StreamCheckpoint) -> ExofsResult<Self> {
        config.validate()?;
        Ok(Self {
            config,
            checkpoint,
            report: StreamExportReport::new(),
        })
    }

    /// Starts export of a list of blobs to the sink.
    ///
    /// `blob_ids`: sorted list of BlobIds to export.
    /// `tombstones`: list of deleted BlobIds since last session.
    ///
    /// RECUR-01: two independent while loops.
    pub fn run<P: BlobDataProvider, S: StreamSink>(
        &mut self,
        provider: &P,
        sink: &mut S,
        blob_ids: &[[u8; 32]],
        tombstones: &[[u8; 32]],
        mut filter: StreamFilter,
    ) -> ExofsResult<StreamExportReport> {
        let start = self.checkpoint.next_index;
        let mut idx = start;

        // ── Phase 1: Active blobs ─────────────────────────────────────────────
        while idx < blob_ids.len() {
            // max_blobs limit
            if self.config.max_blobs > 0
                && self.report.blobs_exported >= self.config.max_blobs as u64
            {
                break;
            }

            let blob_id = &blob_ids[idx];

            // Blob epoch
            let epoch = match provider.blob_epoch(blob_id) {
                Ok(e) => e,
                Err(_) => {
                    self.report.record_error();
                    self.report.record_skipped();
                    idx = idx.wrapping_add(1);
                    continue;
                }
            };

            // Read blob data
            let data = match provider.provide_blob(blob_id) {
                Ok(d) => d,
                Err(_) => {
                    self.report.record_error();
                    self.report.record_skipped();
                    idx = idx.wrapping_add(1);
                    continue;
                }
            };

            let size = data.len() as u64;

            // Filter
            if !filter.should_export(blob_id, epoch, size) {
                self.report.record_skipped();
                idx = idx.wrapping_add(1);
                continue;
            }

            // Verify blob_id (RULE 11: blake3 of raw data)
            if self.config.verify_blob_id {
                let computed = inline_blake3(data);
                if computed != *blob_id {
                    self.report.record_error();
                    self.report.record_skipped();
                    idx = idx.wrapping_add(1);
                    continue;
                }
            }

            // Emit to sink
            if let Err(_) = sink.begin_entry(blob_id, size) {
                self.report.record_error();
                idx = idx.wrapping_add(1);
                continue;
            }

            // Send by blocks (ARITH-02 + RECUR-01: while loop)
            let block = self.config.block_size as usize;
            let mut offset = 0usize;
            let mut ok = true;
            while offset < data.len() {
                let end = (offset.saturating_add(block)).min(data.len());
                if sink.write_chunk(&data[offset..end]).is_err() {
                    self.report.record_error();
                    ok = false;
                    break;
                }
                offset = end;
            }

            if ok {
                let _ = sink.end_entry();
                self.report.record_exported(size);
                self.checkpoint.advance(*blob_id, epoch.value());
            }
            idx = idx.wrapping_add(1);
        }

        // ── Phase 2: Tombstones ───────────────────────────────────────────────
        if self.config.include_tombstones {
            let mut tj = 0usize;
            while tj < tombstones.len() {
                let bid = &tombstones[tj];
                // Emit size 0 entry to signify deletion
                if sink.begin_entry(bid, 0).is_ok() {
                    let _ = sink.end_entry();
                    self.report.record_tombstone();
                } else {
                    self.report.record_error();
                }
                tj = tj.wrapping_add(1);
            }
        }

        self.report.bytes_written_to_sink = sink.bytes_written();
        self.report.is_complete = true;
        Ok(self.report)
    }

    pub fn checkpoint(&self) -> &StreamCheckpoint {
        &self.checkpoint
    }
    pub fn report(&self) -> &StreamExportReport {
        &self.report
    }
}

// ─── Export Batch with Checkpoint ────────────────────────────────────────────

/// State of an export batch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BatchState {
    Pending,
    Running,
    Completed,
    Failed,
    Paused,
}

/// Streaming export batch with checkpoint support.
pub struct StreamExportBatch {
    pub batch_id: u32,
    pub state: BatchState,
    pub config: StreamExportConfig,
    pub checkpoint: StreamCheckpoint,
    pub report: StreamExportReport,
    /// List of blob_ids to export in this batch.
    blob_ids: Vec<[u8; 32]>,
    /// Tombstones of the batch.
    tombstones: Vec<[u8; 32]>,
}

impl StreamExportBatch {
    pub fn new(batch_id: u32, config: StreamExportConfig) -> Self {
        Self {
            batch_id,
            state: BatchState::Pending,
            config,
            checkpoint: StreamCheckpoint::new(),
            report: StreamExportReport::new(),
            blob_ids: Vec::new(),
            tombstones: Vec::new(),
        }
    }

    /// Adds a blob_id to the batch — OOM-02: try_reserve.
    pub fn add_blob(&mut self, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.blob_ids
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.blob_ids.push(blob_id);
        Ok(())
    }

    /// Adds a tombstone to the batch — OOM-02: try_reserve.
    pub fn add_tombstone(&mut self, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.tombstones
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.tombstones.push(blob_id);
        Ok(())
    }

    /// Runs the batch export.
    pub fn run<P: BlobDataProvider, S: StreamSink>(
        &mut self,
        provider: &P,
        sink: &mut S,
    ) -> ExofsResult<StreamExportReport> {
        self.state = BatchState::Running;
        let filter = StreamFilter::accept_all();
        let mut exporter = StreamExporter::new(self.config)?;
        match exporter.run(provider, sink, &self.blob_ids, &self.tombstones, filter) {
            Ok(report) => {
                self.report = report;
                self.checkpoint = *exporter.checkpoint();
                self.state = if report.has_errors() {
                    BatchState::Failed
                } else {
                    BatchState::Completed
                };
                Ok(report)
            }
            Err(e) => {
                self.state = BatchState::Failed;
                self.report.record_error();
                Err(e)
            }
        }
    }

    pub fn blob_count(&self) -> usize {
        self.blob_ids.len()
    }
    pub fn is_done(&self) -> bool {
        matches!(self.state, BatchState::Completed | BatchState::Failed)
    }
}

// ─── Minimal Inline blake3 (no_std) ─────────────────────────────────────────

/// Simplified blake3 hash (production: replace with crate::fs::exofs::dedup::content_hash).
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
        out[k * 4] = w[0];
        out[k * 4 + 1] = w[1];
        out[k * 4 + 2] = w[2];
        out[k * 4 + 3] = w[3];
        k = k.wrapping_add(1);
    }
    out
}

// ─── Test VecStreamSink ────────────────────────────────────────────────────

/// Implementation of StreamSink to Vec<u8> (for tests).
pub struct VecStreamSink {
    buf: Vec<u8>,
    entries: u32,
    written: u64,
}

impl VecStreamSink {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            entries: 0,
            written: 0,
        }
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }
    pub fn entry_count(&self) -> u32 {
        self.entries
    }
}

impl StreamSink for VecStreamSink {
    fn write_chunk(&mut self, data: &[u8]) -> ExofsResult<()> {
        self.buf
            .try_reserve(data.len())
            .map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(data);
        self.written = self.written.saturating_add(data.len() as u64);
        Ok(())
    }
    fn begin_entry(&mut self, _blob_id: &[u8; 32], _size: u64) -> ExofsResult<()> {
        self.entries = self.entries.saturating_add(1);
        Ok(())
    }
    fn end_entry(&mut self) -> ExofsResult<()> {
        Ok(())
    }
    fn bytes_written(&self) -> u64 {
        self.written
    }
}

// ─── MockBlobDataProvider ─────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
struct MockProvider {
    blobs: Vec<([u8; 32], Vec<u8>, EpochId)>,
}

#[cfg(test)]
impl MockProvider {
    fn new() -> Self {
        Self { blobs: Vec::new() }
    }

    fn add(&mut self, data: &[u8], epoch: EpochId) -> [u8; 32] {
        let id = inline_blake3(data);
        let mut v = Vec::new();
        v.extend_from_slice(data);
        self.blobs.push((id, v, epoch));
        id
    }
}

#[cfg(test)]
impl BlobDataProvider for MockProvider {
    fn provide_blob(&self, blob_id: &[u8; 32]) -> ExofsResult<&[u8]> {
        for (id, data, _) in &self.blobs {
            if id == blob_id {
                return Ok(data.as_slice());
            }
        }
        Err(ExofsError::NotFound)
    }
    fn blob_epoch(&self, blob_id: &[u8; 32]) -> ExofsResult<EpochId> {
        for (id, _, epoch) in &self.blobs {
            if id == blob_id {
                return Ok(*epoch);
            }
        }
        Err(ExofsError::NotFound)
    }
    fn blob_exists(&self, blob_id: &[u8; 32]) -> bool {
        self.blobs.iter().any(|(id, _, _)| id == blob_id)
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
    fn test_filter_accept_all() {
        let mut f = StreamFilter::accept_all();
        assert!(f.should_export(&make_id(1), EpochId(5), 100));
        assert_eq!(f.accepted(), 1);
        assert_eq!(f.rejected(), 0);
    }

    #[test]
    fn test_filter_epoch_range() {
        let mut f = StreamFilter::epoch_range(3, 7);
        assert!(!f.should_export(&make_id(1), EpochId(1), 0));
        assert!(f.should_export(&make_id(1), EpochId(5), 0));
        assert!(!f.should_export(&make_id(1), EpochId(10), 0));
        assert_eq!(f.accepted(), 1);
        assert_eq!(f.rejected(), 2);
    }

    #[test]
    fn test_filter_max_size() {
        let mut f = StreamFilter::new(FilterMode::MaxSize(100));
        assert!(f.should_export(&make_id(1), EpochId(1), 50));
        assert!(!f.should_export(&make_id(1), EpochId(1), 200));
    }

    #[test]
    fn test_config_validate() {
        let ok = StreamExportConfig::default(1);
        assert!(ok.validate().is_ok());
        let bad = StreamExportConfig {
            block_size: 0,
            ..StreamExportConfig::default(1)
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn test_checkpoint_advance() {
        let mut ck = StreamCheckpoint::new();
        assert!(!ck.valid);
        ck.advance(make_id(5), 7);
        assert!(ck.valid);
        assert_eq!(ck.next_index, 1);
        assert_eq!(ck.last_epoch, 7);
    }

    #[test]
    fn test_stream_report_methods() {
        let mut r = StreamExportReport::new();
        r.record_exported(512);
        r.record_exported(256);
        r.record_tombstone();
        assert_eq!(r.blobs_exported, 2);
        assert_eq!(r.bytes_exported, 768);
        assert_eq!(r.tombstones_emitted, 1);
        assert_eq!(r.total_entries(), 3);
        assert!(!r.has_errors());
    }

    #[test]
    fn test_export_zero_blobs() {
        let provider = MockProvider::new();
        let cfg = StreamExportConfig::default(1);
        let mut exporter = StreamExporter::new(cfg).test_expect("ok");
        let mut sink = VecStreamSink::new();
        let report = exporter
            .run(&provider, &mut sink, &[], &[], StreamFilter::accept_all())
            .test_expect("ok");
        assert_eq!(report.blobs_exported, 0);
        assert!(report.is_complete);
    }

    #[test]
    fn test_export_single_blob() {
        let mut provider = MockProvider::new();
        let bid = provider.add(b"hello exofs", EpochId(3));
        let cfg = StreamExportConfig {
            verify_blob_id: false,
            ..StreamExportConfig::default(1)
        };
        let mut exporter = StreamExporter::new(cfg).test_expect("ok");
        let mut sink = VecStreamSink::new();
        let ids = [bid];
        let report = exporter
            .run(&provider, &mut sink, &ids, &[], StreamFilter::accept_all())
            .test_expect("ok");
        assert_eq!(report.blobs_exported, 1);
        assert_eq!(sink.entry_count(), 1);
        assert!(report.is_complete);
    }

    #[test]
    fn test_export_multiple_blobs() {
        let mut provider = MockProvider::new();
        let mut ids = Vec::new();
        for i in 0u8..5 {
            let data = [i; 64]; // 64 bytes of tag i
            ids.push(provider.add(&data, EpochId(i as u64)));
        }
        let cfg = StreamExportConfig {
            verify_blob_id: false,
            ..StreamExportConfig::default(1)
        };
        let mut exporter = StreamExporter::new(cfg).test_expect("ok");
        let mut sink = VecStreamSink::new();
        let report = exporter
            .run(&provider, &mut sink, &ids, &[], StreamFilter::accept_all())
            .test_expect("ok");
        assert_eq!(report.blobs_exported, 5);
        assert_eq!(report.bytes_exported, 320);
    }

    #[test]
    fn test_export_with_tombstones() {
        let provider = MockProvider::new();
        let cfg = StreamExportConfig::default(1);
        let mut exporter = StreamExporter::new(cfg).test_expect("ok");
        let mut sink = VecStreamSink::new();
        let tombs = [make_id(11), make_id(22)];
        let report = exporter
            .run(
                &provider,
                &mut sink,
                &[],
                &tombs,
                StreamFilter::accept_all(),
            )
            .test_expect("ok");
        assert_eq!(report.tombstones_emitted, 2);
    }

    #[test]
    fn test_export_max_blobs_limit() {
        let mut provider = MockProvider::new();
        let mut ids = Vec::new();
        for i in 0u8..10 {
            ids.push(provider.add(&[i; 32], EpochId(1)));
        }
        let cfg = StreamExportConfig {
            verify_blob_id: false,
            max_blobs: 4,
            ..StreamExportConfig::default(1)
        };
        let mut exporter = StreamExporter::new(cfg).test_expect("ok");
        let mut sink = VecStreamSink::new();
        let report = exporter
            .run(&provider, &mut sink, &ids, &[], StreamFilter::accept_all())
            .test_expect("ok");
        assert_eq!(report.blobs_exported, 4);
    }

    #[test]
    fn test_batch_add_blobs() {
        let cfg = StreamExportConfig::default(5);
        let mut batch = StreamExportBatch::new(1, cfg);
        for i in 0u8..3 {
            batch.add_blob(make_id(i)).test_expect("ok");
        }
        assert_eq!(batch.blob_count(), 3);
        assert_eq!(batch.state, BatchState::Pending);
    }

    #[test]
    fn test_batch_run() {
        let mut provider = MockProvider::new();
        let mut actual_ids = Vec::new();
        for i in 0u8..3 {
            actual_ids.push(provider.add(&[i; 128], EpochId(1)));
        }
        let cfg = StreamExportConfig {
            verify_blob_id: false,
            ..StreamExportConfig::default(1)
        };
        let mut batch = StreamExportBatch::new(1, cfg);
        for id in &actual_ids {
            batch.add_blob(*id).test_expect("ok");
        }
        let mut sink = VecStreamSink::new();
        let report = batch.run(&provider, &mut sink).test_expect("ok");
        assert_eq!(report.blobs_exported, 3);
        assert!(batch.is_done());
    }

    #[test]
    fn test_filter_exclude_large() {
        let mut f = StreamFilter::new(FilterMode::ExcludeLarge(100));
        assert!(f.should_export(&make_id(1), EpochId(1), 50));
        assert!(f.should_export(&make_id(1), EpochId(1), 100));
        assert!(!f.should_export(&make_id(1), EpochId(1), 200));
    }

    #[test]
    fn test_inline_blake3_deterministic() {
        let a = inline_blake3(b"test");
        let b = inline_blake3(b"test");
        assert_eq!(a, b);
        let c = inline_blake3(b"other");
        assert_ne!(a, c);
    }

    #[test]
    fn test_vec_stream_sink() {
        let mut sink = VecStreamSink::new();
        sink.begin_entry(&[0u8; 32], 4).test_expect("ok");
        sink.write_chunk(b"data").test_expect("ok");
        sink.end_entry().test_expect("ok");
        assert_eq!(sink.bytes_written(), 4);
        assert_eq!(sink.entry_count(), 1);
        assert_eq!(sink.as_slice(), b"data");
    }
}
