//! snapshot_restore.rs — Restoration of an ExoFS snapshot
//!
//! Restoration pipeline: reads each blob from a source, verifies
//! its identifier (HASH-02: verify_blob_id on RAW data), then
//! writes it via a RestoreSink.
//!
//! Spec rules:
//!   HASH-02  : verify_blob_id on RAW data before writing
//!   WRITE-02 : verify bytes_written == expected
//!   OOM-02   : try_reserve before each push

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use super::snapshot::flags;
use super::snapshot_list::SNAPSHOT_LIST;
use crate::fs::exofs::core::blob_id::verify_blob_id;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult, SnapshotId};

// ─────────────────────────────────────────────────────────────
// Traits
// ─────────────────────────────────────────────────────────────

/// Restoration destination: receives blobs and persists them
pub trait RestoreSink: Send + Sync {
    /// Writes a blob; must return `ShortWrite` if incomplete
    fn write_blob(&mut self, blob_id: BlobId, data: &[u8]) -> ExofsResult<usize>;
    /// Finalizes restoration (flush, commit)
    fn finalize(&mut self) -> ExofsResult<()>;
    /// Rollback on error
    fn abort(&mut self);
}

/// Blob source for restoration
pub trait SnapshotBlobSource: Send + Sync {
    /// Reads a blob by its identifier
    fn read_blob(&self, snap_id: SnapshotId, blob_id: BlobId) -> ExofsResult<Vec<u8>>;
    /// Enumerates blob identifiers of a snapshot
    fn list_blobs(&self, snap_id: SnapshotId) -> ExofsResult<Vec<BlobId>>;
}

// ─────────────────────────────────────────────────────────────
// Restoration options
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct RestoreOptions {
    /// Verifies integrity (HASH-02) of each blob
    pub verify_integrity: bool,
    /// Continues on non-fatal errors (corrupted blob)
    pub resilient: bool,
    /// Max number of blobs to restore (0 = all)
    pub max_blobs: usize,
    /// Ignore blobs already present in destination
    pub skip_existing: bool,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            verify_integrity: true,
            resilient: false,
            max_blobs: 0,
            skip_existing: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Restoration result
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RestoreResult {
    pub snap_id: SnapshotId,
    pub n_blobs_ok: u64,
    pub n_blobs_error: u64,
    pub n_blobs_skip: u64,
    pub bytes_restored: u64,
    pub errors: Vec<RestoreError>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct RestoreError {
    pub blob_id: BlobId,
    pub kind: RestoreErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreErrorKind {
    ReadFailed,
    IntegrityFailed,
    WriteFailed,
    ShortWrite,
}

// ─────────────────────────────────────────────────────────────
// SnapshotRestore
// ─────────────────────────────────────────────────────────────

pub struct SnapshotRestore {
    aborted: AtomicBool,
}

impl SnapshotRestore {
    pub const fn new() -> Self {
        Self {
            aborted: AtomicBool::new(false),
        }
    }

    // ── Entry point ──────────────────────────────────────────

    /// Starts full restoration of a snapshot
    pub fn restore<S: SnapshotBlobSource, D: RestoreSink>(
        &self,
        snap_id: SnapshotId,
        source: &S,
        dest: &mut D,
        opts: RestoreOptions,
    ) -> ExofsResult<RestoreResult> {
        // Verify that snapshot exists
        let _snap_ref = SNAPSHOT_LIST.get_ref(snap_id)?;

        // Mark snapshot as being restored
        SNAPSHOT_LIST.set_flags(snap_id, flags::RESTORING)?;

        let result = self.run_pipeline(snap_id, source, dest, opts);

        // Clear RESTORING flag
        let _ = SNAPSHOT_LIST.clear_flags(snap_id, flags::RESTORING);

        match result {
            Ok(r) => Ok(r),
            Err(e) => {
                dest.abort();
                Err(e)
            }
        }
    }

    // ── Internal pipeline ────────────────────────────────────────────

    fn run_pipeline<S: SnapshotBlobSource, D: RestoreSink>(
        &self,
        snap_id: SnapshotId,
        source: &S,
        dest: &mut D,
        opts: RestoreOptions,
    ) -> ExofsResult<RestoreResult> {
        let blob_ids = source.list_blobs(snap_id)?;
        let mut result = RestoreResult {
            snap_id,
            n_blobs_ok: 0,
            n_blobs_error: 0,
            n_blobs_skip: 0,
            bytes_restored: 0,
            errors: Vec::new(),
            truncated: false,
        };

        let limit = if opts.max_blobs > 0 {
            opts.max_blobs
        } else {
            usize::MAX
        };

        for (i, blob_id) in blob_ids.iter().enumerate() {
            if self.aborted.load(Ordering::Acquire) {
                dest.abort();
                return Err(ExofsError::Shutdown);
            }
            if i >= limit {
                result.truncated = true;
                break;
            }

            match self.restore_one_blob(*blob_id, snap_id, source, dest, opts) {
                Ok(bytes) => {
                    result.n_blobs_ok += 1;
                    result.bytes_restored = result.bytes_restored.saturating_add(bytes as u64);
                }
                Err(kind) => {
                    result.n_blobs_error += 1;
                    result
                        .errors
                        .try_reserve(1)
                        .map_err(|_| ExofsError::NoMemory)?;
                    result.errors.push(RestoreError {
                        blob_id: *blob_id,
                        kind,
                    });
                    if !opts.resilient {
                        dest.abort();
                        return Err(ExofsError::ChecksumMismatch);
                    }
                }
            }
        }

        // WRITE-02: finalize only if zero errors or resilient mode
        if result.n_blobs_error == 0 || opts.resilient {
            dest.finalize()?;
        } else {
            dest.abort();
            return Err(ExofsError::ChecksumMismatch);
        }

        Ok(result)
    }

    fn restore_one_blob<S: SnapshotBlobSource, D: RestoreSink>(
        &self,
        blob_id: BlobId,
        snap_id: SnapshotId,
        source: &S,
        dest: &mut D,
        opts: RestoreOptions,
    ) -> Result<usize, RestoreErrorKind> {
        // Read from source
        let data = source
            .read_blob(snap_id, blob_id)
            .map_err(|_| RestoreErrorKind::ReadFailed)?;

        // HASH-02: verify integrity on RAW data
        if opts.verify_integrity {
            if !verify_blob_id(&blob_id, &data) {
                return Err(RestoreErrorKind::IntegrityFailed);
            }
        }

        let expected = data.len();

        // WRITE-02: verify bytes_written == expected
        let written = dest
            .write_blob(blob_id, &data)
            .map_err(|_| RestoreErrorKind::WriteFailed)?;
        if written != expected {
            return Err(RestoreErrorKind::ShortWrite);
        }

        Ok(written)
    }

    // ── Abort ────────────────────────────────────────────────────────

    /// Aborts ongoing restoration (callable from another thread)
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::Release);
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::Acquire)
    }

    pub fn reset(&self) {
        self.aborted.store(false, Ordering::Release);
    }
}

// ─────────────────────────────────────────────────────────────
// Null RestoreSink (useful for dry-run / tests)
// ─────────────────────────────────────────────────────────────

/// Sink that counts bytes without writing anything (dry-run)
pub struct NullRestoreSink {
    pub bytes_received: u64,
    pub blobs_received: u64,
}

impl NullRestoreSink {
    pub fn new() -> Self {
        Self {
            bytes_received: 0,
            blobs_received: 0,
        }
    }
}

impl RestoreSink for NullRestoreSink {
    fn write_blob(&mut self, _: BlobId, data: &[u8]) -> ExofsResult<usize> {
        self.bytes_received = self.bytes_received.saturating_add(data.len() as u64);
        self.blobs_received += 1;
        Ok(data.len()) // WRITE-02: correct return value
    }
    fn finalize(&mut self) -> ExofsResult<()> {
        Ok(())
    }
    fn abort(&mut self) {}
}

// ─────────────────────────────────────────────────────────────
// In-memory SnapshotBlobSource (tests)
// ─────────────────────────────────────────────────────────────

/// In-memory source (for tests)
pub struct MemBlobSource {
    entries: alloc::collections::BTreeMap<[u8; 32], Vec<u8>>,
    snap_blobs: alloc::collections::BTreeMap<u64, Vec<BlobId>>,
}

impl MemBlobSource {
    pub fn new() -> Self {
        Self {
            entries: alloc::collections::BTreeMap::new(),
            snap_blobs: alloc::collections::BTreeMap::new(),
        }
    }

    /// OOM-02: try_reserve before push
    pub fn add_blob(
        &mut self,
        snap_id: SnapshotId,
        blob_id: BlobId,
        data: Vec<u8>,
    ) -> ExofsResult<()> {
        self.entries.insert(*blob_id.as_bytes(), data);
        let list = self.snap_blobs.entry(snap_id.0).or_insert_with(Vec::new);
        list.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        list.push(blob_id);
        Ok(())
    }
}

impl SnapshotBlobSource for MemBlobSource {
    fn read_blob(&self, _: SnapshotId, blob_id: BlobId) -> ExofsResult<Vec<u8>> {
        self.entries
            .get(blob_id.as_bytes())
            .cloned()
            .ok_or(ExofsError::NotFound)
    }

    fn list_blobs(&self, snap_id: SnapshotId) -> ExofsResult<Vec<BlobId>> {
        Ok(self.snap_blobs.get(&snap_id.0).cloned().unwrap_or_default())
    }
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::super::reset_for_test;
    use super::super::snapshot::{make_snapshot_name, Snapshot};
    use super::super::snapshot_list::{SnapshotList, SNAPSHOT_LIST};
    use super::*;
    use crate::fs::exofs::core::blob_id::compute_blob_id;
    use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, SnapshotId};

    fn push_snap(_list: &SnapshotList, id: u64) {
        SNAPSHOT_LIST
            .register(Snapshot {
                id: SnapshotId(id),
                epoch_id: EpochId(1),
                parent_id: None,
                root_blob: BlobId([0u8; 32]),
                created_at: 0,
                n_blobs: 1,
                total_bytes: 0,
                flags: 0,
                blob_catalog_offset: DiskOffset(0),
                blob_catalog_size: 0,
                name: make_snapshot_name(b"restore-test"),
            })
            .test_unwrap();
    }

    #[test]
    fn restore_null_sink_ok() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 1);
        let raw = b"hello world";
        let bid = compute_blob_id(raw);
        let mut source = MemBlobSource::new();
        source
            .add_blob(SnapshotId(1), bid, raw.to_vec())
            .test_unwrap();
        let mut sink = NullRestoreSink::new();
        let restore = SnapshotRestore::new();
        let result = restore
            .restore(SnapshotId(1), &source, &mut sink, RestoreOptions::default())
            .test_unwrap();
        assert_eq!(result.n_blobs_ok, 1);
        assert_eq!(result.n_blobs_error, 0);
        assert_eq!(sink.bytes_received, raw.len() as u64);
    }

    #[test]
    fn restore_corrupted_blob_detected() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 2);
        let raw = b"original data";
        let bid = compute_blob_id(raw);
        let mut source = MemBlobSource::new();
        // Injects corrupted data (HASH-02: verification must fail)
        source
            .add_blob(SnapshotId(2), bid, b"corrupted data".to_vec())
            .test_unwrap();
        let mut sink = NullRestoreSink::new();
        let restore = SnapshotRestore::new();
        let err = restore.restore(SnapshotId(2), &source, &mut sink, RestoreOptions::default());
        assert!(err.is_err());
    }

    #[test]
    fn resilient_mode_continues_on_error() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 3);
        let bid = compute_blob_id(b"real");
        let mut source = MemBlobSource::new();
        source
            .add_blob(SnapshotId(3), bid, b"fake".to_vec())
            .test_unwrap();
        let mut sink = NullRestoreSink::new();
        let opts = RestoreOptions {
            verify_integrity: true,
            resilient: true,
            ..Default::default()
        };
        let restore = SnapshotRestore::new();
        let result = restore
            .restore(SnapshotId(3), &source, &mut sink, opts)
            .test_unwrap();
        assert_eq!(result.n_blobs_error, 1);
    }

    #[test]
    fn abort_stops_restore() {
        let _guard = reset_for_test();
        let restore = SnapshotRestore::new();
        restore.abort();
        assert!(restore.is_aborted());
        restore.reset();
        assert!(!restore.is_aborted());
    }
}
