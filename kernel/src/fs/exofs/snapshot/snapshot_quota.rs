//! snapshot_quota.rs — ExoFS snapshot quota management
//!
//! Manages per-snapshot quotas (maximum size) and global quotas
//! (total bytes, maximum number of snapshots).
//!
//! Spec rules:
//!   ARITH-02 : checked_add for accumulators
//!   OOM-02   : try_reserve before push

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::snapshot::flags;
use super::snapshot_list::SNAPSHOT_LIST;
use crate::fs::exofs::core::{ExofsError, ExofsResult, SnapshotId};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────

pub static SNAPSHOT_QUOTA: SnapshotQuotaTable = SnapshotQuotaTable::new_const();

// ─────────────────────────────────────────────────────────────
// Per-snapshot quota
// ─────────────────────────────────────────────────────────────

/// Quota entry for a specific snapshot
#[derive(Debug, Clone, Copy)]
pub struct SnapshotQuotaEntry {
    pub snap_id: SnapshotId,
    /// Maximum allowed bytes (0 = unlimited)
    pub max_bytes: u64,
    /// Maximum number of blobs (0 = unlimited)
    pub max_blobs: u64,
    /// Currently used bytes
    pub used_bytes: u64,
    /// Currently used blobs
    pub used_blobs: u64,
}

impl SnapshotQuotaEntry {
    pub fn new(snap_id: SnapshotId, max_bytes: u64, max_blobs: u64) -> Self {
        Self {
            snap_id,
            max_bytes,
            max_blobs,
            used_bytes: 0,
            used_blobs: 0,
        }
    }

    /// Checks if an allocation of `bytes` / `blobs` is allowed
    pub fn can_allocate(&self, bytes: u64, blobs: u64) -> bool {
        let new_bytes = self.used_bytes.saturating_add(bytes);
        let new_blobs = self.used_blobs.saturating_add(blobs);
        (self.max_bytes == 0 || new_bytes <= self.max_bytes)
            && (self.max_blobs == 0 || new_blobs <= self.max_blobs)
    }

    /// Returns true if quota is exceeded
    pub fn is_exceeded(&self) -> bool {
        (self.max_bytes > 0 && self.used_bytes > self.max_bytes)
            || (self.max_blobs > 0 && self.used_blobs > self.max_blobs)
    }

    /// Utilization ratio (0.0 – 1.0) — bytes
    pub fn usage_ratio_bytes(&self) -> f64 {
        if self.max_bytes == 0 {
            return 0.0;
        }
        (self.used_bytes as f64) / (self.max_bytes as f64)
    }
}

// ─────────────────────────────────────────────────────────────
// Global quota
// ─────────────────────────────────────────────────────────────

/// Global quota policy
#[derive(Debug, Clone, Copy)]
pub struct GlobalQuotaPolicy {
    /// Maximum total bytes (0 = unlimited)
    pub max_total_bytes: u64,
    /// Maximum number of snapshots (0 = unlimited)
    pub max_snap_count: usize,
    /// Default max size of a snapshot (0 = unlimited)
    pub default_snap_max_bytes: u64,
}

impl Default for GlobalQuotaPolicy {
    fn default() -> Self {
        Self {
            max_total_bytes: 0,
            max_snap_count: 0,
            default_snap_max_bytes: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotQuotaTable
// ─────────────────────────────────────────────────────────────

pub struct SnapshotQuotaTable {
    entries: SpinLock<alloc::collections::BTreeMap<u64, SnapshotQuotaEntry>>,
    policy: SpinLock<GlobalQuotaPolicy>,
    global_used_bytes: AtomicU64,
    global_snap_count: AtomicUsize,
}

impl SnapshotQuotaTable {
    pub const fn new_const() -> Self {
        Self {
            entries: SpinLock::new(alloc::collections::BTreeMap::new()),
            policy: SpinLock::new(GlobalQuotaPolicy {
                max_total_bytes: 0,
                max_snap_count: 0,
                default_snap_max_bytes: 0,
            }),
            global_used_bytes: AtomicU64::new(0),
            global_snap_count: AtomicUsize::new(0),
        }
    }

    // ── Global policy ────────────────────────────────────────

    pub fn set_policy(&self, p: GlobalQuotaPolicy) {
        *self.policy.lock() = p;
    }

    pub fn get_policy(&self) -> GlobalQuotaPolicy {
        *self.policy.lock()
    }

    // ── Per-snapshot quota ───────────────────────────────────

    /// Registers a quota for a snapshot (OOM-02: try_reserve)
    pub fn set(&self, snap_id: SnapshotId, max_bytes: u64, max_blobs: u64) -> ExofsResult<()> {
        // Verify snapshot exists
        let _ = SNAPSHOT_LIST.get_ref(snap_id)?;

        let entry = SnapshotQuotaEntry::new(snap_id, max_bytes, max_blobs);
        let mut guard = self.entries.lock();
        guard.insert(snap_id.0, entry);
        // Set QUOTA_SET flag in registry
        drop(guard);
        SNAPSHOT_LIST.set_flags(snap_id, flags::QUOTA_SET)?;
        Ok(())
    }

    /// Deletes quota for a snapshot
    pub fn remove(&self, snap_id: SnapshotId) -> ExofsResult<()> {
        let mut guard = self.entries.lock();
        guard.remove(&snap_id.0).ok_or(ExofsError::NotFound)?;
        drop(guard);
        SNAPSHOT_LIST.clear_flags(snap_id, flags::QUOTA_SET)?;
        Ok(())
    }

    // ── Usage update ────────────────────────────────────────

    /// Updates usage statistics for a snapshot.
    /// ARITH-02: checked_add
    pub fn update_usage(
        &self,
        snap_id: SnapshotId,
        bytes_delta: i64,
        blobs_delta: i64,
    ) -> ExofsResult<()> {
        let mut guard = self.entries.lock();
        if let Some(entry) = guard.get_mut(&snap_id.0) {
            if bytes_delta >= 0 {
                entry.used_bytes = entry
                    .used_bytes
                    .checked_add(bytes_delta as u64)
                    .ok_or(ExofsError::Overflow)?;
            } else {
                entry.used_bytes = entry.used_bytes.saturating_sub((-bytes_delta) as u64);
            }
            if blobs_delta >= 0 {
                entry.used_blobs = entry
                    .used_blobs
                    .checked_add(blobs_delta as u64)
                    .ok_or(ExofsError::Overflow)?;
            } else {
                entry.used_blobs = entry.used_blobs.saturating_sub((-blobs_delta) as u64);
            }
        }
        // Update global counter
        if bytes_delta >= 0 {
            loop {
                let old = self.global_used_bytes.load(Ordering::Acquire);
                let new = old
                    .checked_add(bytes_delta as u64)
                    .ok_or(ExofsError::Overflow)?;
                if self
                    .global_used_bytes
                    .compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    break;
                }
            }
        } else {
            let sub = (-bytes_delta) as u64;
            loop {
                let old = self.global_used_bytes.load(Ordering::Acquire);
                let new = old.saturating_sub(sub);
                if self
                    .global_used_bytes
                    .compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    break;
                }
            }
        }
        Ok(())
    }

    // ── Pre-allocation verification ─────────────────────────

    /// Checks if an allocation is allowed (per-snap + global quota)
    pub fn check_allocation(&self, snap_id: SnapshotId, bytes: u64, blobs: u64) -> ExofsResult<()> {
        // ── Global quota ────────────────────────────────────────
        let policy = *self.policy.lock();
        if policy.max_total_bytes > 0 {
            let new_total = self
                .global_used_bytes
                .load(Ordering::Acquire)
                .checked_add(bytes)
                .ok_or(ExofsError::Overflow)?;
            if new_total > policy.max_total_bytes {
                return Err(ExofsError::InvalidSize);
            }
        }
        if policy.max_snap_count > 0 {
            let count = self.global_snap_count.load(Ordering::Acquire);
            if count >= policy.max_snap_count {
                return Err(ExofsError::BufferFull);
            }
        }

        // ── Per-snapshot quota ──────────────────────────────────
        let guard = self.entries.lock();
        if let Some(entry) = guard.get(&snap_id.0) {
            if !entry.can_allocate(bytes, blobs) {
                return Err(ExofsError::InvalidSize);
            }
        } else if policy.default_snap_max_bytes > 0 {
            // Default quota if no explicit entry
            let snap = SNAPSHOT_LIST
                .get(snap_id)
                .map_err(|_| ExofsError::NotFound)?;
            let new_bytes = snap
                .total_bytes
                .checked_add(bytes)
                .ok_or(ExofsError::Overflow)?;
            if new_bytes > policy.default_snap_max_bytes {
                return Err(ExofsError::InvalidSize);
            }
        }
        Ok(())
    }

    // ── Exceeded snapshots ───────────────────────────────────

    /// Returns snapshots whose quota is exceeded
    pub fn exceeded_snapshots(&self) -> ExofsResult<Vec<SnapshotId>> {
        let guard = self.entries.lock();
        let mut out: Vec<SnapshotId> = Vec::new();
        for e in guard.values() {
            if e.is_exceeded() {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(e.snap_id);
            }
        }
        Ok(out)
    }

    // ── Queries ─────────────────────────────────────────────

    pub fn get(&self, snap_id: SnapshotId) -> Option<SnapshotQuotaEntry> {
        let guard = self.entries.lock();
        guard.get(&snap_id.0).copied()
    }

    pub fn all_entries(&self) -> ExofsResult<Vec<SnapshotQuotaEntry>> {
        let guard = self.entries.lock();
        let mut out: Vec<SnapshotQuotaEntry> = Vec::new();
        out.try_reserve(guard.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for e in guard.values() {
            out.push(*e);
        }
        Ok(out)
    }

    // ── Global statistics ───────────────────────────────────

    pub fn global_used_bytes(&self) -> u64 {
        self.global_used_bytes.load(Ordering::Acquire)
    }
    pub fn global_snap_count(&self) -> usize {
        self.global_snap_count.load(Ordering::Acquire)
    }

    pub fn register_snap_creation(&self) -> ExofsResult<()> {
        let policy = *self.policy.lock();
        if policy.max_snap_count > 0 {
            let current = self.global_snap_count.load(Ordering::Acquire);
            if current >= policy.max_snap_count {
                return Err(ExofsError::BufferFull);
            }
        }
        self.global_snap_count.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    pub fn register_snap_deletion(&self) {
        let _ = self
            .global_snap_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                Some(v.saturating_sub(1))
            });
    }

    pub fn clear(&self) {
        let mut guard = self.entries.lock();
        guard.clear();
        self.global_used_bytes.store(0, Ordering::Release);
        self.global_snap_count.store(0, Ordering::Release);
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
    use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, SnapshotId};

    fn push_snap(_list: &SnapshotList, id: u64) {
        SNAPSHOT_LIST
            .register(Snapshot {
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
                name: make_snapshot_name(b"q-test"),
            })
            .test_unwrap();
    }

    #[test]
    fn set_and_get_quota() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 1);
        let qt = SnapshotQuotaTable::new_const();
        qt.set(SnapshotId(1), 1024, 10).test_unwrap();
        let entry = qt.get(SnapshotId(1)).test_unwrap();
        assert_eq!(entry.max_bytes, 1024);
        assert_eq!(entry.max_blobs, 10);
    }

    #[test]
    fn can_allocate_within_quota() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 2);
        let qt = SnapshotQuotaTable::new_const();
        qt.set(SnapshotId(2), 4096, 5).test_unwrap();
        assert!(qt.check_allocation(SnapshotId(2), 1000, 2).is_ok());
    }

    #[test]
    fn allocation_exceeds_quota_rejected() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 3);
        let qt = SnapshotQuotaTable::new_const();
        qt.set(SnapshotId(3), 100, 0).test_unwrap();
        let err = qt.check_allocation(SnapshotId(3), 200, 0);
        assert!(matches!(err, Err(ExofsError::InvalidSize)));
    }

    #[test]
    fn global_quota_respected() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 4);
        let qt = SnapshotQuotaTable::new_const();
        qt.set_policy(GlobalQuotaPolicy {
            max_total_bytes: 500,
            ..Default::default()
        });
        qt.update_usage(SnapshotId(4), 400, 0).test_unwrap();
        let err = qt.check_allocation(SnapshotId(4), 200, 0);
        assert!(matches!(err, Err(ExofsError::InvalidSize)));
    }

    #[test]
    fn update_usage_negative_saturates() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 5);
        let qt = SnapshotQuotaTable::new_const();
        qt.set(SnapshotId(5), 0, 0).test_unwrap();
        qt.update_usage(SnapshotId(5), 100, 5).test_unwrap();
        qt.update_usage(SnapshotId(5), -200, -10).test_unwrap(); // does not overflow
        let e = qt.get(SnapshotId(5)).test_unwrap();
        assert_eq!(e.used_bytes, 0);
    }

    #[test]
    fn exceeded_snapshots_detected() {
        let _guard = reset_for_test();
        let list = SnapshotList::new_const();
        push_snap(&list, 6);
        let qt = SnapshotQuotaTable::new_const();
        qt.set(SnapshotId(6), 50, 0).test_unwrap();
        qt.update_usage(SnapshotId(6), 100, 0).test_unwrap();
        let exceeded = qt.exceeded_snapshots().test_unwrap();
        assert!(exceeded.iter().any(|id| id.0 == 6));
    }
}
