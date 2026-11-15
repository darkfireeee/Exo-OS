//! snapshot_delete.rs — Safe deletion of an ExoFS snapshot
//!
//! Verifies deletion conditions (protection, active children,
//! ongoing mount) before removing the snapshot from the registry.
//!
//! Spec rules:
//!   OOM-02   : try_reserve before each Vec::push
//!   ARITH-02 : checked_add for counters

extern crate alloc;
use alloc::vec::Vec;

use super::snapshot::Snapshot;
use super::snapshot_list::SNAPSHOT_LIST;
use crate::fs::exofs::core::{ExofsError, ExofsResult, SnapshotId};

// ─────────────────────────────────────────────────────────────
// Deletion options
// ─────────────────────────────────────────────────────────────

/// Options passed to `SnapshotDeleter::delete`
#[derive(Debug, Clone, Copy)]
pub struct DeleteOptions {
    /// Recursively deletes children before parent
    pub cascade: bool,
    /// Forces deletion even if the snapshot is protected
    pub force: bool,
    /// Does not delete if the snapshot is mounted
    pub skip_mounted: bool,
}

impl Default for DeleteOptions {
    fn default() -> Self {
        Self {
            cascade: false,
            force: false,
            skip_mounted: true,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// Result
// ─────────────────────────────────────────────────────────────

/// Result of a deletion
#[derive(Debug, Clone)]
pub struct DeleteResult {
    /// Deleted identifier
    pub id: SnapshotId,
    /// Freed bytes
    pub freed_bytes: u64,
    /// Number of child snapshots also deleted (cascade)
    pub n_cascade: u32,
    /// Identifiers of deleted children (cascade)
    pub cascade_ids: Vec<SnapshotId>,
}

// ─────────────────────────────────────────────────────────────
// Rejection reasons
// ─────────────────────────────────────────────────────────────

/// Reason why deletion is impossible
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteDenyReason {
    /// Snapshot protected against deletion
    Protected,
    /// Children exist (without cascade enabled)
    HasChildren,
    /// Snapshot currently mounted
    Mounted,
    /// Snapshot currently being restored
    Restoring,
    /// Snapshot currently being streamed
    Streaming,
    /// Snapshot not found
    NotFound,
}

impl From<DeleteDenyReason> for ExofsError {
    fn from(r: DeleteDenyReason) -> Self {
        match r {
            DeleteDenyReason::Protected => ExofsError::InvalidState,
            DeleteDenyReason::HasChildren => ExofsError::InvalidState,
            DeleteDenyReason::Mounted => ExofsError::InvalidState,
            DeleteDenyReason::Restoring => ExofsError::InvalidState,
            DeleteDenyReason::Streaming => ExofsError::InvalidState,
            DeleteDenyReason::NotFound => ExofsError::NotFound,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotDeleter
// ─────────────────────────────────────────────────────────────

pub struct SnapshotDeleter;

impl SnapshotDeleter {
    // ── Main entry point ─────────────────────────────────────

    /// Deletes a snapshot after validation
    pub fn delete(id: SnapshotId, opts: DeleteOptions) -> ExofsResult<DeleteResult> {
        // Load snapshot for validation
        let snap = SNAPSHOT_LIST.get(id).map_err(|_| ExofsError::NotFound)?;

        // Check preconditions
        Self::check_preconditions(&snap, opts)?;

        // Handle cascade deletion
        let mut total_freed: u64 = 0;
        let mut cascade_ids: Vec<SnapshotId> = Vec::new();
        let mut n_cascade: u32 = 0;

        if opts.cascade {
            Self::cascade_delete(id, opts, &mut total_freed, &mut cascade_ids, &mut n_cascade)?;
        } else {
            // Check that no children exist
            let children = SNAPSHOT_LIST.children_of(id)?;
            if !children.is_empty() {
                return Err(ExofsError::from(DeleteDenyReason::HasChildren));
            }
        }

        // Remove the main snapshot
        let removed = SNAPSHOT_LIST.remove(id)?;
        total_freed = total_freed.saturating_add(removed.total_bytes);

        Ok(DeleteResult {
            id,
            freed_bytes: total_freed,
            n_cascade,
            cascade_ids,
        })
    }

    // ── Multiple deletions ───────────────────────────────────

    /// Deletes multiple snapshots (without individual cascade)
    ///
    /// OOM-02: try_reserve before push
    pub fn delete_batch(ids: &[SnapshotId], opts: DeleteOptions) -> ExofsResult<Vec<DeleteResult>> {
        let mut results: Vec<DeleteResult> = Vec::new();
        results
            .try_reserve(ids.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for &id in ids {
            let r = Self::delete(id, opts)?;
            results.push(r);
        }
        Ok(results)
    }

    // ── Precondition check ──────────────────────────────────

    pub fn check_preconditions(snap: &Snapshot, opts: DeleteOptions) -> ExofsResult<()> {
        if snap.is_protected() && !opts.force {
            return Err(ExofsError::from(DeleteDenyReason::Protected));
        }
        if snap.is_mounted() && opts.skip_mounted {
            return Err(ExofsError::from(DeleteDenyReason::Mounted));
        }
        if snap.is_restoring() {
            return Err(ExofsError::from(DeleteDenyReason::Restoring));
        }
        if snap.is_streaming() {
            return Err(ExofsError::from(DeleteDenyReason::Streaming));
        }
        Ok(())
    }

    // ── Recursive deletion ───────────────────────────────────────────

    /// Recursively deletes children iteratively (RECUR-01).
    /// Validates preconditions on all descendants before any deletion.
    fn cascade_delete(
        parent_id: SnapshotId,
        opts: DeleteOptions,
        total_freed: &mut u64,
        cascade_ids: &mut Vec<SnapshotId>,
        n_cascade: &mut u32,
    ) -> ExofsResult<()> {
        let mut worklist = Vec::new();
        worklist.try_reserve(16).map_err(|_| ExofsError::NoMemory)?;
        for child in SNAPSHOT_LIST.children_of(parent_id)? {
            worklist.push(child);
        }

        let mut to_delete = Vec::new();
        to_delete.try_reserve(worklist.len()).map_err(|_| ExofsError::NoMemory)?;

        while let Some(current) = worklist.pop() {
            to_delete.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            to_delete.push(current);
            let grand_children = SNAPSHOT_LIST.children_of(current)?;
            for gc in grand_children {
                worklist.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                worklist.push(gc);
            }
        }

        // Verify preconditions for all descendants before deletion
        for &child_id in &to_delete {
            let child = SNAPSHOT_LIST.get(child_id).map_err(|_| ExofsError::NotFound)?;
            Self::check_preconditions(&child, opts)?;
        }

        // Effective deletion in reverse order (leaves before branches)
        while let Some(child_id) = to_delete.pop() {
            let removed = SNAPSHOT_LIST.remove(child_id)?;
            *total_freed = total_freed.saturating_add(removed.total_bytes);
            cascade_ids.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            cascade_ids.push(child_id);
            *n_cascade = n_cascade.checked_add(1).ok_or(ExofsError::Overflow)?;
        }

        Ok(())
    }

    // ── Diagnostic utilities ─────────────────────────────────────────

    /// Returns true if the snapshot is eligible for deletion
    pub fn can_delete(id: SnapshotId, opts: DeleteOptions) -> bool {
        let Ok(snap) = SNAPSHOT_LIST.get(id) else {
            return false;
        };
        if Self::check_preconditions(&snap, opts).is_err() {
            return false;
        }
        // Without cascade: verify absence of children
        if !opts.cascade {
            let Ok(children) = SNAPSHOT_LIST.children_of(id) else {
                return false;
            };
            if !children.is_empty() {
                return false;
            }
        }
        true
    }

    /// Returns the denial reason (or None if deletion is possible)
    pub fn deny_reason(id: SnapshotId, opts: DeleteOptions) -> Option<DeleteDenyReason> {
        let Ok(snap) = SNAPSHOT_LIST.get(id) else {
            return Some(DeleteDenyReason::NotFound);
        };
        if snap.is_protected() && !opts.force {
            return Some(DeleteDenyReason::Protected);
        }
        if snap.is_mounted() && opts.skip_mounted {
            return Some(DeleteDenyReason::Mounted);
        }
        if snap.is_restoring() {
            return Some(DeleteDenyReason::Restoring);
        }
        if snap.is_streaming() {
            return Some(DeleteDenyReason::Streaming);
        }
        if !opts.cascade {
            let Ok(children) = SNAPSHOT_LIST.children_of(id) else {
                return Some(DeleteDenyReason::NotFound);
            };
            if !children.is_empty() {
                return Some(DeleteDenyReason::HasChildren);
            }
        }
        None
    }

    /// Recursively estimates total bytes that would be freed (dry-run)
    pub fn estimate_cascade_freed(id: SnapshotId) -> ExofsResult<u64> {
        let snap = SNAPSHOT_LIST.get(id).map_err(|_| ExofsError::NotFound)?;
        let mut total = snap.total_bytes;
        let children = SNAPSHOT_LIST.children_of(id)?;
        for child_id in children {
            let child_freed = Self::estimate_cascade_freed(child_id)?;
            total = total.checked_add(child_freed).ok_or(ExofsError::Overflow)?;
        }
        Ok(total)
    }

    /// Returns the ordered deletion list (children before parents — dry-run)
    pub fn deletion_order(id: SnapshotId) -> ExofsResult<Vec<SnapshotId>> {
        let mut order: Vec<SnapshotId> = Vec::new();
        Self::collect_order(id, &mut order)?;
        Ok(order)
    }

    fn collect_order(id: SnapshotId, order: &mut Vec<SnapshotId>) -> ExofsResult<()> {
        let children = SNAPSHOT_LIST.children_of(id)?;
        for child_id in children {
            Self::collect_order(child_id, order)?;
        }
        order.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        // OOM-02 already ensured via try_reserve
        order.push(id);
        Ok(())
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
    use super::super::snapshot::{flags, make_snapshot_name};
    use super::super::snapshot_list::{SnapshotList, SNAPSHOT_LIST};
    use super::*;
    use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId};

    fn push_snap(_list: &SnapshotList, id: u64, bytes: u64, parent: Option<u64>, flags: u32) {
        use super::super::snapshot::Snapshot;
        SNAPSHOT_LIST
            .register(Snapshot {
                id: SnapshotId(id),
                epoch_id: EpochId(1),
                parent_id: parent.map(SnapshotId),
                root_blob: BlobId([0u8; 32]),
                created_at: 100 + id,
                n_blobs: 0,
                total_bytes: bytes,
                flags,
                blob_catalog_offset: DiskOffset(0),
                blob_catalog_size: 0,
                name: make_snapshot_name(b"t"),
            })
            .test_unwrap();
    }

    #[test]
    fn cannot_delete_protected_without_force() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 1, 0, None, flags::PROTECTED);
        let snap = SNAPSHOT_LIST.get(SnapshotId(1)).test_unwrap();
        let opts = DeleteOptions::default();
        let err = SnapshotDeleter::check_preconditions(&snap, opts);
        assert!(matches!(err, Err(ExofsError::InvalidState)));
    }

    #[test]
    fn force_bypasses_protected() {
        let snap = super::super::snapshot::Snapshot {
            id: SnapshotId(1),
            epoch_id: EpochId(1),
            parent_id: None,
            root_blob: BlobId([0u8; 32]),
            created_at: 0,
            n_blobs: 0,
            total_bytes: 0,
            flags: flags::PROTECTED,
            blob_catalog_offset: DiskOffset(0),
            blob_catalog_size: 0,
            name: make_snapshot_name(b"t"),
        };
        let opts = DeleteOptions {
            force: true,
            ..Default::default()
        };
        assert!(SnapshotDeleter::check_preconditions(&snap, opts).is_ok());
    }

    #[test]
    fn deny_reason_has_children() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 1, 0, None, 0);
        push_snap(&list, 2, 0, Some(1), 0);
        let reason = SnapshotDeleter::deny_reason(SnapshotId(1), DeleteOptions::default());
        assert_eq!(reason, Some(DeleteDenyReason::HasChildren));
    }

    #[test]
    fn estimate_cascade_freed_correct() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 10, 1000, None, 0);
        push_snap(&list, 11, 2000, Some(10), 0);
        let freed = SnapshotDeleter::estimate_cascade_freed(SnapshotId(10)).test_unwrap();
        assert_eq!(freed, 3000);
    }

    #[test]
    fn deletion_order_children_first() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 1, 0, None, 0);
        push_snap(&list, 2, 0, Some(1), 0);
        push_snap(&list, 3, 0, Some(2), 0);
        let order = SnapshotDeleter::deletion_order(SnapshotId(1)).test_unwrap();
        // 3 must appear before 2, which must appear before 1
        let pos = |id: u64| order.iter().position(|s| s.0 == id).test_unwrap();
        assert!(pos(3) < pos(2));
        assert!(pos(2) < pos(1));
    }
}
