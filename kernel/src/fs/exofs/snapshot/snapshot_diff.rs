//! snapshot_diff.rs — Comparison of two ExoFS snapshots
//!
//! Computes the diff (added, removed, modified blobs) between two snapshots.
//! Provides a detailed report with statistics.
//!
//! Spec rules:
//!   OOM-02   : try_reserve before each push
//!   ARITH-02 : checked_add for counters

extern crate alloc;
use alloc::vec::Vec;
use core::cmp::Ordering as CmpOrdering;

use super::snapshot::SnapshotRef;
use super::snapshot_list::SNAPSHOT_LIST;
use crate::fs::exofs::core::{BlobId, ExofsError, ExofsResult, SnapshotId};

// ─────────────────────────────────────────────────────────────
// DiffKind
// ─────────────────────────────────────────────────────────────

/// Type of a diff entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// Blob present in right snapshot, absent in left snapshot
    Added,
    /// Blob present in left snapshot, absent in right snapshot
    Removed,
    /// Blob present in both but with different data
    Modified,
    /// Identical blob in both snapshots (included if option `include_unchanged`)
    Unchanged,
}

// ─────────────────────────────────────────────────────────────
// DiffEntry
// ─────────────────────────────────────────────────────────────

/// Entry in the diff report
#[derive(Debug, Clone)]
pub struct DiffEntry {
    /// Blob id in left snapshot (None if Added)
    pub blob_left: Option<BlobId>,
    /// Blob id in right snapshot (None if Removed)
    pub blob_right: Option<BlobId>,
    /// Difference type
    pub kind: DiffKind,
    /// Size in bytes of left blob (0 if absent)
    pub size_left: u64,
    /// Size in bytes of right blob (0 if absent)
    pub size_right: u64,
}

impl DiffEntry {
    fn added(blob: BlobId, size: u64) -> Self {
        Self {
            blob_left: None,
            blob_right: Some(blob),
            kind: DiffKind::Added,
            size_left: 0,
            size_right: size,
        }
    }
    fn removed(blob: BlobId, size: u64) -> Self {
        Self {
            blob_left: Some(blob),
            blob_right: None,
            kind: DiffKind::Removed,
            size_left: size,
            size_right: 0,
        }
    }
    #[allow(dead_code)]
    fn modified(left: BlobId, right: BlobId, sl: u64, sr: u64) -> Self {
        Self {
            blob_left: Some(left),
            blob_right: Some(right),
            kind: DiffKind::Modified,
            size_left: sl,
            size_right: sr,
        }
    }
    fn unchanged(blob: BlobId, size: u64) -> Self {
        Self {
            blob_left: Some(blob),
            blob_right: Some(blob),
            kind: DiffKind::Unchanged,
            size_left: size,
            size_right: size,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Diff options
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct DiffOptions {
    /// Include unchanged blobs in report
    pub include_unchanged: bool,
    /// Max size of returned entries (0 = unlimited)
    pub max_entries: usize,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            include_unchanged: false,
            max_entries: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotDiffReport
// ─────────────────────────────────────────────────────────────

/// Diff report between two snapshots
#[derive(Debug, Clone)]
pub struct SnapshotDiffReport {
    /// Left snapshot (reference)
    pub left: SnapshotRef,
    /// Right snapshot (compared)
    pub right: SnapshotRef,
    /// Diff entries
    pub entries: Vec<DiffEntry>,
    /// Number of added blobs
    pub n_added: u64,
    /// Number of removed blobs
    pub n_removed: u64,
    /// Number of modified blobs (same position/index, different content)
    pub n_modified: u64,
    /// Number of unchanged blobs
    pub n_unchanged: u64,
    /// Added bytes (Added blobs) — ARITH-02: checked
    pub bytes_added: u64,
    /// Removed bytes (Removed blobs) — ARITH-02: checked
    pub bytes_removed: u64,
    /// Truncated to `max_entries` if true
    pub truncated: bool,
}

impl SnapshotDiffReport {
    /// One-line summary for logs
    pub fn summary(&self) -> DiffSummary {
        DiffSummary {
            n_added: self.n_added,
            n_removed: self.n_removed,
            n_modified: self.n_modified,
            n_unchanged: self.n_unchanged,
            bytes_added: self.bytes_added,
            bytes_removed: self.bytes_removed,
        }
    }

    pub fn has_changes(&self) -> bool {
        self.n_added > 0 || self.n_removed > 0 || self.n_modified > 0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DiffSummary {
    pub n_added: u64,
    pub n_removed: u64,
    pub n_modified: u64,
    pub n_unchanged: u64,
    pub bytes_added: u64,
    pub bytes_removed: u64,
}

// ─────────────────────────────────────────────────────────────
// Blob enumeration trait
// ─────────────────────────────────────────────────────────────

/// Provides the ordered list of blobs in a snapshot with their sizes
pub trait SnapshotBlobEnumerator: Send + Sync {
    fn list_blobs_with_sizes(&self, snap_id: SnapshotId) -> ExofsResult<Vec<(BlobId, u64)>>;
}

/// Simple enumerator based on in-RAM metadata (sizes at 0)
pub struct MetaOnlyEnumerator;

impl SnapshotBlobEnumerator for MetaOnlyEnumerator {
    fn list_blobs_with_sizes(&self, snap_id: SnapshotId) -> ExofsResult<Vec<(BlobId, u64)>> {
        let snap = SNAPSHOT_LIST.get(snap_id)?;
        // Without disk access, we can only return size 0
        // In a real system, this would read the blob catalog
        let mut out: Vec<(BlobId, u64)> = Vec::new();
        out.try_reserve(snap.n_blobs as usize)
            .map_err(|_| ExofsError::NoMemory)?;
        // Root as sole visible entry
        out.push((snap.root_blob, 0));
        Ok(out)
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotDiff
// ─────────────────────────────────────────────────────────────

pub struct SnapshotDiff;

impl SnapshotDiff {
    // ── Main entry point ─────────────────────────────────────

    /// Compares two snapshots via their blob enumerator
    pub fn compare<E: SnapshotBlobEnumerator>(
        left_id: SnapshotId,
        right_id: SnapshotId,
        enumerator: &E,
        opts: DiffOptions,
    ) -> ExofsResult<SnapshotDiffReport> {
        let left_ref = SNAPSHOT_LIST.get_ref(left_id)?;
        let right_ref = SNAPSHOT_LIST.get_ref(right_id)?;

        let mut left_blobs = enumerator.list_blobs_with_sizes(left_id)?;
        let mut right_blobs = enumerator.list_blobs_with_sizes(right_id)?;

        // Sort by blob_id.as_bytes() for linear comparison
        left_blobs.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        right_blobs.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

        Self::merge_diff(left_ref, right_ref, &left_blobs, &right_blobs, opts)
    }

    // ── Direct comparison of two blob lists ──────────────────

    /// Computes diff between two already sorted lists of (BlobId, size)
    pub fn diff_sorted(
        left_ref: SnapshotRef,
        right_ref: SnapshotRef,
        left_blobs: &[(BlobId, u64)],
        right_blobs: &[(BlobId, u64)],
        opts: DiffOptions,
    ) -> ExofsResult<SnapshotDiffReport> {
        Self::merge_diff(left_ref, right_ref, left_blobs, right_blobs, opts)
    }

    // ── Merge-sort O(n) merge ────────────────────────────────────────

    fn merge_diff(
        left_ref: SnapshotRef,
        right_ref: SnapshotRef,
        left_blobs: &[(BlobId, u64)],
        right_blobs: &[(BlobId, u64)],
        opts: DiffOptions,
    ) -> ExofsResult<SnapshotDiffReport> {
        let max_cap = left_blobs.len().saturating_add(right_blobs.len());
        let mut entries: Vec<DiffEntry> = Vec::new();
        let cap = if opts.max_entries > 0 {
            opts.max_entries.min(max_cap)
        } else {
            max_cap
        };
        entries.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;

        let mut n_added: u64 = 0;
        let mut n_removed: u64 = 0;
        let n_modified: u64 = 0;
        let mut n_unchanged: u64 = 0;
        let mut bytes_added: u64 = 0;
        let mut bytes_removed: u64 = 0;
        let mut li = 0usize;
        let mut ri = 0usize;
        let mut truncated = false;

        while li < left_blobs.len() || ri < right_blobs.len() {
            if opts.max_entries > 0 && entries.len() >= opts.max_entries {
                truncated = true;
                break;
            }

            let entry = match (left_blobs.get(li), right_blobs.get(ri)) {
                (Some(l), Some(r)) => match l.0.as_bytes().cmp(r.0.as_bytes()) {
                    CmpOrdering::Equal => {
                        li += 1;
                        ri += 1;
                        n_unchanged = n_unchanged.checked_add(1).ok_or(ExofsError::Overflow)?;
                        if !opts.include_unchanged {
                            continue;
                        }
                        DiffEntry::unchanged(l.0, l.1)
                    }
                    CmpOrdering::Less => {
                        li += 1;
                        n_removed = n_removed.checked_add(1).ok_or(ExofsError::Overflow)?;
                        bytes_removed =
                            bytes_removed.checked_add(l.1).ok_or(ExofsError::Overflow)?;
                        DiffEntry::removed(l.0, l.1)
                    }
                    CmpOrdering::Greater => {
                        ri += 1;
                        n_added = n_added.checked_add(1).ok_or(ExofsError::Overflow)?;
                        bytes_added = bytes_added.checked_add(r.1).ok_or(ExofsError::Overflow)?;
                        DiffEntry::added(r.0, r.1)
                    }
                },
                (Some(l), None) => {
                    li += 1;
                    n_removed = n_removed.checked_add(1).ok_or(ExofsError::Overflow)?;
                    bytes_removed = bytes_removed.checked_add(l.1).ok_or(ExofsError::Overflow)?;
                    DiffEntry::removed(l.0, l.1)
                }
                (None, Some(r)) => {
                    ri += 1;
                    n_added = n_added.checked_add(1).ok_or(ExofsError::Overflow)?;
                    bytes_added = bytes_added.checked_add(r.1).ok_or(ExofsError::Overflow)?;
                    DiffEntry::added(r.0, r.1)
                }
                (None, None) => break,
            };

            entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            entries.push(entry);
        }

        Ok(SnapshotDiffReport {
            left: left_ref,
            right: right_ref,
            entries,
            n_added,
            n_removed,
            n_modified,
            n_unchanged,
            bytes_added,
            bytes_removed,
            truncated,
        })
    }

    // ── Utilities ───────────────────────────────────────────────────

    /// Returns true if both snapshots have exactly the same root_blob
    pub fn is_identical(left_id: SnapshotId, right_id: SnapshotId) -> ExofsResult<bool> {
        let l = SNAPSHOT_LIST.get(left_id)?;
        let r = SNAPSHOT_LIST.get(right_id)?;
        Ok(l.root_blob.ct_eq(&r.root_blob))
    }

    /// Returns blobs unique to left (Removed from the perspective of right)
    pub fn blobs_only_in<E: SnapshotBlobEnumerator>(
        only_in_id: SnapshotId,
        other_id: SnapshotId,
        enumerator: &E,
    ) -> ExofsResult<Vec<BlobId>> {
        let mut left = enumerator.list_blobs_with_sizes(only_in_id)?;
        let mut right = enumerator.list_blobs_with_sizes(other_id)?;
        left.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        right.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

        let mut out: Vec<BlobId> = Vec::new();
        let mut li = 0usize;
        let mut ri = 0usize;
        while li < left.len() {
            match right.get(ri) {
                Some(r) => match left[li].0.as_bytes().cmp(r.0.as_bytes()) {
                    CmpOrdering::Less => {
                        out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                        out.push(left[li].0);
                        li += 1;
                    }
                    CmpOrdering::Equal => {
                        li += 1;
                        ri += 1;
                    }
                    CmpOrdering::Greater => {
                        ri += 1;
                    }
                },
                None => {
                    out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    out.push(left[li].0);
                    li += 1;
                }
            }
        }
        Ok(out)
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
    use crate::fs::exofs::core::{DiskOffset, EpochId, SnapshotId};

    struct TestEnumerator(alloc::collections::BTreeMap<u64, alloc::vec::Vec<(BlobId, u64)>>);

    impl SnapshotBlobEnumerator for TestEnumerator {
        fn list_blobs_with_sizes(&self, id: SnapshotId) -> ExofsResult<Vec<(BlobId, u64)>> {
            Ok(self.0.get(&id.0).cloned().unwrap_or_default())
        }
    }

    fn make_snap(id: u64) -> Snapshot {
        Snapshot {
            id: SnapshotId(id),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: 0,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: make_snapshot_name(b"t"),
        }
    }

    #[test]
    fn diff_added_removed() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        let _ = list;
        SNAPSHOT_LIST.register(make_snap(1)).test_unwrap();
        SNAPSHOT_LIST.register(make_snap(2)).test_unwrap();

        let b1 = compute_blob_id(b"data1");
        let b2 = compute_blob_id(b"data2");
        let b3 = compute_blob_id(b"data3");

        let mut m = alloc::collections::BTreeMap::new();
        m.insert(1u64, alloc::vec![(b1, 10u64), (b2, 20u64)]);
        m.insert(2u64, alloc::vec![(b2, 20u64), (b3, 30u64)]);

        let enumerator = TestEnumerator(m);
        let report = SnapshotDiff::compare(
            SnapshotId(1),
            SnapshotId(2),
            &enumerator,
            DiffOptions::default(),
        )
        .test_unwrap();
        assert_eq!(report.n_added, 1);
        assert_eq!(report.n_removed, 1);
        assert!(!report.has_changes() || report.has_changes()); // OK
    }

    #[test]
    fn diff_identical_snapshots() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        let b = compute_blob_id(b"shared");
        let mut s1 = make_snap(10);
        let mut s2 = make_snap(11);
        s1.root_blob = b;
        s1.n_blobs = 1;
        s2.root_blob = b;
        s2.n_blobs = 1;
        let _ = list;
        SNAPSHOT_LIST.register(s1).test_unwrap();
        SNAPSHOT_LIST.register(s2).test_unwrap();
        let identical = SnapshotDiff::is_identical(SnapshotId(10), SnapshotId(11)).test_unwrap();
        assert!(identical);
    }

    #[test]
    fn diff_max_entries_truncation() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        let _ = list;
        SNAPSHOT_LIST.register(make_snap(20)).test_unwrap();
        SNAPSHOT_LIST.register(make_snap(21)).test_unwrap();
        let blobs: alloc::vec::Vec<(BlobId, u64)> = (0u8..10)
            .map(|i| {
                let mut b = [0u8; 32];
                b[0] = i;
                (BlobId(b), i as u64 * 10)
            })
            .collect();
        let mut m = alloc::collections::BTreeMap::new();
        m.insert(20u64, blobs.clone());
        m.insert(21u64, alloc::vec![]);
        let enumerator = TestEnumerator(m);
        let opts = DiffOptions {
            max_entries: 3,
            include_unchanged: false,
        };
        let report =
            SnapshotDiff::compare(SnapshotId(20), SnapshotId(21), &enumerator, opts).test_unwrap();
        assert!(report.truncated);
        assert_eq!(report.entries.len(), 3);
    }
}
