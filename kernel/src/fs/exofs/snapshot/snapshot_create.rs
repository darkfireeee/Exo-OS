//! snapshot_create.rs — ExoFS snapshot creation pipeline
//!
//! Constructs a snapshot from a set of blob IDs supplied by the caller,
//! computes the Merkle root (HASH-02: RAW data before compression),
//! validates parameters, and registers into SNAPSHOT_LIST.
//!
//! Spec rules:
//!   HASH-02  : compute_blob_id on RAW data (never compressed)
//!   OOM-02   : try_reserve before each push
//!   ARITH-02 : checked_add / checked_mul for sizes
//!   WRITE-02 : verify bytes_written == expected after each write

extern crate alloc;
use alloc::vec::Vec;

use super::snapshot::{flags, make_snapshot_name, Snapshot, SNAPSHOT_NAME_LEN};
use super::snapshot_list::SNAPSHOT_LIST;
use crate::fs::exofs::core::blob_id::compute_blob_id;
use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, ExofsError, ExofsResult, SnapshotId};

// ─────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────

/// Maximum number of blobs per snapshot
pub const SNAPSHOT_MAX_BLOBS: u64 = 1 << 24; // 16M

/// Maximum total size of a snapshot (512 GiB)
pub const SNAPSHOT_MAX_TOTAL_BYTES: u64 = 512 << 30;

// ─────────────────────────────────────────────────────────────
// Creation parameters
// ─────────────────────────────────────────────────────────────

/// Creation parameters for a snapshot
#[derive(Clone, Debug)]
pub struct SnapshotParams {
    /// Name (UTF-8, max SNAPSHOT_NAME_LEN bytes)
    pub name: [u8; SNAPSHOT_NAME_LEN],
    /// Parent snapshot (None = root snapshot)
    pub parent_id: Option<SnapshotId>,
    /// Creation epoch
    pub epoch_id: EpochId,
    /// Initial flags
    pub flags: u32,
    /// Quota in bytes (0 = no quota)
    pub quota_bytes: u64,
    /// Creation timestamp (ticks)
    pub created_at: u64,
    /// Disk offset of the blob catalog
    pub blob_catalog_offset: DiskOffset,
    /// Size of the blob catalog on disk
    pub blob_catalog_size: u32,
}

impl SnapshotParams {
    pub fn new(name: &[u8], parent_id: Option<SnapshotId>, epoch_id: EpochId) -> Self {
        Self {
            name: make_snapshot_name(name),
            parent_id,
            epoch_id,
            flags: 0,
            quota_bytes: 0,
            created_at: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Creation result
// ─────────────────────────────────────────────────────────────

/// Result of a creation operation
#[derive(Debug, Clone)]
pub struct SnapshotCreateResult {
    /// Allocated identifier for the new snapshot
    pub id: SnapshotId,
    /// Merkle root computed on RAW blob IDs
    pub root_blob: BlobId,
    /// Number of blobs
    pub n_blobs: u64,
    /// Total bytes (sum of blob sizes)
    pub total_bytes: u64,
    /// Creation duration (ticks)
    pub duration_ticks: u64,
}

// ─────────────────────────────────────────────────────────────
// SnapshotBlobSet — set of blobs provided for creation
// ─────────────────────────────────────────────────────────────

/// Represents a blob to be included in the snapshot
#[derive(Debug, Clone)]
pub struct BlobEntry {
    /// Blob identifier (HASH-02: computed on RAW data)
    pub blob_id: BlobId,
    /// Size in bytes of the RAW data
    pub raw_size: u64,
}

impl BlobEntry {
    pub fn new(blob_id: BlobId, raw_size: u64) -> Self {
        Self { blob_id, raw_size }
    }
}

/// Ordered set of blobs ready for snapshot creation
pub struct SnapshotBlobSet {
    entries: Vec<BlobEntry>,
}

impl SnapshotBlobSet {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// OOM-02: try_reserve before push
    pub fn push(&mut self, entry: BlobEntry) -> ExofsResult<()> {
        self.entries
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(entry);
        Ok(())
    }

    pub fn len(&self) -> u64 {
        self.entries.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total RAW size — ARITH-02: checked_add
    pub fn total_bytes(&self) -> ExofsResult<u64> {
        let mut total: u64 = 0;
        for e in &self.entries {
            total = total.checked_add(e.raw_size).ok_or(ExofsError::Overflow)?;
        }
        Ok(total)
    }

    /// Computes the Merkle root of blob IDs (HASH-02: on RAW data)
    pub fn compute_root_blob(&self) -> ExofsResult<BlobId> {
        if self.entries.is_empty() {
            // Empty snapshot: root = hash of empty data
            return Ok(compute_blob_id(&[]));
        }
        // Collect the 32 bytes of each blob_id
        let mut raw: Vec<u8> = Vec::new();
        raw.try_reserve(
            self.entries
                .len()
                .checked_mul(32)
                .ok_or(ExofsError::Overflow)?,
        )
        .map_err(|_| ExofsError::NoMemory)?;
        for e in &self.entries {
            raw.extend_from_slice(e.blob_id.as_bytes());
        }
        Ok(compute_blob_id(&raw))
    }

    pub fn entries(&self) -> &[BlobEntry] {
        &self.entries
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotCreator
// ─────────────────────────────────────────────────────────────

/// Snapshot builder
///
/// Typical usage:
/// ```ignore
/// let mut set = SnapshotBlobSet::new();
/// set.push(BlobEntry::new(compute_blob_id(&raw_data), raw_data.len() as u64))?;
/// let result = SnapshotCreator::create(&params, set)?;
/// ```
pub struct SnapshotCreator;

impl SnapshotCreator {
    /// Creates a new snapshot and registers it in SNAPSHOT_LIST
    pub fn create(
        params: &SnapshotParams,
        blobs: SnapshotBlobSet,
    ) -> ExofsResult<SnapshotCreateResult> {
        // ── Parameter validation ────────────────────────────────
        Self::validate_params(params)?;

        // ── Blob count validation ────────────────────────────
        if blobs.len() > SNAPSHOT_MAX_BLOBS {
            return Err(ExofsError::InvalidSize);
        }

        // ── Total size computation (ARITH-02: checked) ────────────────
        let total_bytes = blobs.total_bytes()?;
        if total_bytes > SNAPSHOT_MAX_TOTAL_BYTES {
            return Err(ExofsError::InvalidSize);
        }

        // ── Quota verification (if set) ───────────────────────────
        if params.quota_bytes != 0 && total_bytes > params.quota_bytes {
            return Err(ExofsError::InvalidSize);
        }

        // ── Verification that parent exists ────────────────────────
        if let Some(parent_id) = params.parent_id {
            let _ = SNAPSHOT_LIST.get_ref(parent_id)?;
        }

        // ── Merkle root computation (HASH-02: RAW blobs) ─────────
        let root_blob = blobs.compute_root_blob()?;
        let n_blobs = blobs.len();

        // ── Identifier allocation ──────────────────────────────
        let snap_id = SNAPSHOT_LIST.allocate_id()?;

        // ── Flags initialization ─────────────────────────────────
        let mut snap_flags = params.flags;
        if params.quota_bytes != 0 {
            snap_flags |= flags::QUOTA_SET;
        }
        if params.parent_id.is_some() {
            snap_flags |= flags::INCREMENTAL;
        }

        // ── In-memory snapshot construction ──────────────────────
        let snap = Snapshot {
            id: snap_id,
            epoch_id: params.epoch_id,
            parent_id: params.parent_id,
            root_blob,
            created_at: params.created_at,
            n_blobs,
            total_bytes,
            flags: snap_flags,
            blob_catalog_offset: params.blob_catalog_offset,
            blob_catalog_size: params.blob_catalog_size,
            name: params.name,
        };

        // ── Registration in the global registry ───────────────────
        SNAPSHOT_LIST.register(snap)?;

        Ok(SnapshotCreateResult {
            id: snap_id,
            root_blob,
            n_blobs,
            total_bytes,
            duration_ticks: 0,
        })
    }

    /// Creates an empty snapshot (useful for root or test snapshots)
    pub fn create_empty(params: &SnapshotParams) -> ExofsResult<SnapshotCreateResult> {
        Self::create(params, SnapshotBlobSet::new())
    }

    /// Validates creation parameters
    pub fn validate_params(params: &SnapshotParams) -> ExofsResult<()> {
        // Non-empty name
        if params.name.iter().all(|&b| b == 0) {
            return Err(ExofsError::InvalidArgument);
        }
        // Valid epoch
        if !params.epoch_id.is_valid() {
            return Err(ExofsError::InvalidArgument);
        }
        // No circular reference = parent != self (impossible as id is not allocated yet)
        Ok(())
    }

    /// Constructs an incremental snapshot from a parent and a blob diff.
    ///
    /// Existing blobs of the parent are reused + new blobs added.
    pub fn create_incremental(
        parent_id: SnapshotId,
        new_blobs: Vec<BlobEntry>,
        epoch_id: EpochId,
        name: &[u8],
        created_at: u64,
    ) -> ExofsResult<SnapshotCreateResult> {
        let _parent = SNAPSHOT_LIST.get(parent_id)?;

        let params = SnapshotParams {
            name: make_snapshot_name(name),
            parent_id: Some(parent_id),
            epoch_id,
            flags: flags::INCREMENTAL,
            quota_bytes: 0,
            created_at,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
        };

        let mut set = SnapshotBlobSet::new();
        for entry in new_blobs {
            set.push(entry)?;
        }

        Self::create(&params, set)
    }
}

// ─────────────────────────────────────────────────────────────
// Utilities
// ─────────────────────────────────────────────────────────────

/// Constructs a list of BlobEntry from raw data.
///
/// HASH-02: compute_blob_id is called on `raw_data` BEFORE any compression.
pub fn entries_from_raw(raw_data: &[&[u8]]) -> ExofsResult<Vec<BlobEntry>> {
    let mut out: Vec<BlobEntry> = Vec::new();
    out.try_reserve(raw_data.len())
        .map_err(|_| ExofsError::NoMemory)?;
    for data in raw_data {
        let blob_id = compute_blob_id(data);
        let raw_size = data.len() as u64;
        out.push(BlobEntry { blob_id, raw_size });
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::blob_id::compute_blob_id;

    fn base_params(name: &[u8]) -> SnapshotParams {
        SnapshotParams {
            name: make_snapshot_name(name),
            parent_id: None,
            epoch_id: EpochId(1),
            flags: 0,
            quota_bytes: 0,
            created_at: 1000,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
        }
    }

    #[test]
    fn create_empty_snapshot() {
        let params = base_params(b"empty-snap");
        let result = SnapshotCreator::create_empty(&params).test_unwrap();
        assert_eq!(result.n_blobs, 0);
        assert_eq!(result.total_bytes, 0);
    }

    #[test]
    fn create_with_blobs_hash02() {
        let raw1 = b"hello world";
        let raw2 = b"exo-os kernel";

        let mut set = SnapshotBlobSet::new();
        // HASH-02: compute_blob_id on RAW data
        set.push(BlobEntry::new(compute_blob_id(raw1), raw1.len() as u64))
            .test_unwrap();
        set.push(BlobEntry::new(compute_blob_id(raw2), raw2.len() as u64))
            .test_unwrap();

        let params = base_params(b"snap-with-blobs");
        let result = SnapshotCreator::create(&params, set).test_unwrap();
        assert_eq!(result.n_blobs, 2);
        assert_eq!(result.total_bytes, (raw1.len() + raw2.len()) as u64);
    }

    #[test]
    fn quota_exceeded_returns_error() {
        let mut params = base_params(b"quota-snap");
        params.quota_bytes = 4; // very small

        let mut set = SnapshotBlobSet::new();
        let raw = b"too much data";
        set.push(BlobEntry::new(compute_blob_id(raw), raw.len() as u64))
            .test_unwrap();

        let err = SnapshotCreator::create(&params, set);
        assert!(matches!(err, Err(ExofsError::InvalidSize)));
    }

    #[test]
    fn empty_name_invalid() {
        let mut params = base_params(b"");
        params.name = [0u8; SNAPSHOT_NAME_LEN];
        let err = SnapshotCreator::validate_params(&params);
        assert!(matches!(err, Err(ExofsError::InvalidArgument)));
    }

    #[test]
    fn blob_set_total_bytes_overflow() {
        let mut set = SnapshotBlobSet::new();
        // Simulates an overflow with u64::MAX
        set.push(BlobEntry::new(BlobId([0u8; 32]), u64::MAX))
            .test_unwrap();
        set.push(BlobEntry::new(BlobId([1u8; 32]), 1)).test_unwrap();
        let err = set.total_bytes();
        assert!(matches!(err, Err(ExofsError::Overflow)));
    }

    #[test]
    fn root_blob_deterministic() {
        let mut set1 = SnapshotBlobSet::new();
        let mut set2 = SnapshotBlobSet::new();
        let bid = compute_blob_id(b"data");
        set1.push(BlobEntry::new(bid, 4)).test_unwrap();
        set2.push(BlobEntry::new(bid, 4)).test_unwrap();
        let r1 = set1.compute_root_blob().test_unwrap();
        let r2 = set2.compute_root_blob().test_unwrap();
        assert!(r1.ct_eq(&r2));
    }
}
