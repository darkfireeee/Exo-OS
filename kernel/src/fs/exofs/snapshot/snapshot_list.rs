//! snapshot_list.rs — Registre mémoire global des snapshots ExoFS
//!
//! Fournit un singleton thread-safe `SNAPSHOT_LIST` qui indexe tous les
//! snapshots actifs en RAM (BTreeMap par SnapshotId + index par nom).
//!
//! Règles spec :
//!   OOM-02   : try_reserve(1) avant chaque Vec::push et BTreeMap::insert
//!   ARITH-02 : checked_add pour compteurs et stats
//!   ONDISK-03: pas de AtomicXxx dans les structs repr(C)

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{ExofsError, ExofsResult, SnapshotId, EpochId};
use super::snapshot::{Snapshot, SnapshotRef, SNAPSHOT_MAX_COUNT, SNAPSHOT_NAME_LEN};

// ─────────────────────────────────────────────────────────────
// Singleton global
// ─────────────────────────────────────────────────────────────

/// Registre global de tous les snapshots actifs
pub static SNAPSHOT_LIST: SnapshotList = SnapshotList::new_const();

// ─────────────────────────────────────────────────────────────
// SnapshotList
// ─────────────────────────────────────────────────────────────

/// Registre thread-safe des snapshots chargés en mémoire
pub struct SnapshotList {
    inner: SpinLock<BTreeMap<u64, Snapshot>>,
    next_id: AtomicU64,
    count: AtomicUsize,
    total_bytes: AtomicU64,
}

impl SnapshotList {
    pub const fn new_const() -> Self {
        Self {
            inner: SpinLock::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            count: AtomicUsize::new(0),
            total_bytes: AtomicU64::new(0),
        }
    }

    // ── Allocation d'id ──────────────────────────────────────────────

    pub fn allocate_id(&self) -> ExofsResult<SnapshotId> {
        let raw = self.next_id.fetch_add(1, Ordering::AcqRel);
        if raw == 0 {
            let _ = self.next_id.fetch_add(1, Ordering::AcqRel);
            return Ok(SnapshotId(1));
        }
        Ok(SnapshotId(raw))
    }

    // ── Enregistrement ───────────────────────────────────────────────

    /// Enregistre un snapshot — OOM-02 : try_reserve avant insert
    pub fn register(&self, snap: Snapshot) -> ExofsResult<()> {
        if self.count.load(Ordering::Acquire) >= SNAPSHOT_MAX_COUNT {
            return Err(ExofsError::BufferFull);
        }
        let id_raw = snap.id.0;
        let snap_bytes = snap.total_bytes;
        let mut guard = self.inner.lock();
        guard.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        if guard.contains_key(&id_raw) {
            return Err(ExofsError::InvalidState);
        }
        guard.insert(id_raw, snap);
        drop(guard);
        self.count.fetch_add(1, Ordering::AcqRel);
        loop {
            let old = self.total_bytes.load(Ordering::Acquire);
            let new = old.checked_add(snap_bytes).ok_or(ExofsError::Overflow)?;
            if self.total_bytes.compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                break;
            }
        }
        Ok(())
    }

    // ── Suppression ──────────────────────────────────────────────────

    pub fn remove(&self, id: SnapshotId) -> ExofsResult<Snapshot> {
        let mut guard = self.inner.lock();
        let snap = guard.remove(&id.0).ok_or(ExofsError::NotFound)?;
        drop(guard);
        self.count.fetch_sub(1, Ordering::AcqRel);
        loop {
            let old = self.total_bytes.load(Ordering::Acquire);
            let new = old.saturating_sub(snap.total_bytes);
            if self.total_bytes.compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                break;
            }
        }
        Ok(snap)
    }

    // ── Lecture ──────────────────────────────────────────────────────

    pub fn get(&self, id: SnapshotId) -> ExofsResult<Snapshot> {
        let guard = self.inner.lock();
        guard.get(&id.0).cloned().ok_or(ExofsError::NotFound)
    }

    pub fn get_ref(&self, id: SnapshotId) -> ExofsResult<SnapshotRef> {
        let guard = self.inner.lock();
        guard.get(&id.0).map(|s| SnapshotRef::from(s)).ok_or(ExofsError::NotFound)
    }

    pub fn find_by_name(&self, name: &[u8]) -> Option<SnapshotRef> {
        let guard = self.inner.lock();
        for snap in guard.values() {
            let len = name.len().min(SNAPSHOT_NAME_LEN);
            if &snap.name[..len] == &name[..len]
                && snap.name[len..].iter().all(|&b| b == 0) {
                return Some(SnapshotRef::from(snap));
            }
        }
        None
    }

    // ── Modification ──────────────────────────────────────────────────

    pub fn mutate<F>(&self, id: SnapshotId, f: F) -> ExofsResult<()>
    where F: FnOnce(&mut Snapshot) -> ExofsResult<()> {
        let mut guard = self.inner.lock();
        let snap = guard.get_mut(&id.0).ok_or(ExofsError::NotFound)?;
        f(snap)
    }

    pub fn set_flags(&self, id: SnapshotId, flags: u32) -> ExofsResult<()> {
        self.mutate(id, |s| { s.flags |= flags; Ok(()) })
    }

    pub fn clear_flags(&self, id: SnapshotId, flags: u32) -> ExofsResult<()> {
        self.mutate(id, |s| { s.flags &= !flags; Ok(()) })
    }

    // ── Énumération ──────────────────────────────────────────────────

    /// OOM-02 : try_reserve avant push
    pub fn all_ids(&self) -> ExofsResult<Vec<SnapshotId>> {
        let guard = self.inner.lock();
        let mut out: Vec<SnapshotId> = Vec::new();
        out.try_reserve(guard.len()).map_err(|_| ExofsError::NoMemory)?;
        for &k in guard.keys() {
            out.push(SnapshotId(k));
        }
        Ok(out)
    }

    pub fn all_refs(&self) -> ExofsResult<Vec<SnapshotRef>> {
        let guard = self.inner.lock();
        let mut out: Vec<SnapshotRef> = Vec::new();
        out.try_reserve(guard.len()).map_err(|_| ExofsError::NoMemory)?;
        for snap in guard.values() {
            out.push(SnapshotRef::from(snap));
        }
        Ok(out)
    }

    pub fn children_of(&self, parent_id: SnapshotId) -> ExofsResult<Vec<SnapshotId>> {
        let guard = self.inner.lock();
        let mut out: Vec<SnapshotId> = Vec::new();
        for snap in guard.values() {
            if snap.parent_id == Some(parent_id) {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(snap.id);
            }
        }
        Ok(out)
    }

    pub fn older_than(&self, now: u64, max_age_ticks: u64) -> ExofsResult<Vec<SnapshotRef>> {
        let guard = self.inner.lock();
        let mut out: Vec<SnapshotRef> = Vec::new();
        for snap in guard.values() {
            if now.saturating_sub(snap.created_at) > max_age_ticks {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(SnapshotRef::from(snap));
            }
        }
        Ok(out)
    }

    // ── Statistiques ─────────────────────────────────────────────────

    pub fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> ListStats {
        let guard = self.inner.lock();
        let mut s = ListStats::default();
        s.count = guard.len();
        for snap in guard.values() {
            use super::snapshot::flags as f;
            if snap.created_at < s.oldest_created_at || s.count == 1 {
                s.oldest_created_at = snap.created_at;
            }
            if snap.created_at > s.newest_created_at { s.newest_created_at = snap.created_at; }
            if snap.flags & f::PROTECTED != 0 { s.n_protected += 1; }
            if snap.flags & f::READONLY  != 0 { s.n_readonly  += 1; }
            if snap.flags & f::ORPHAN    != 0 { s.n_orphan    += 1; }
            s.total_bytes = s.total_bytes.saturating_add(snap.total_bytes);
            if snap.n_blobs     > s.max_blobs_in_one { s.max_blobs_in_one = snap.n_blobs; }
            if snap.total_bytes > s.max_bytes_in_one { s.max_bytes_in_one = snap.total_bytes; }
        }
        s
    }

    // ── Maintenance ──────────────────────────────────────────────────

    pub fn clear(&self) {
        let mut guard = self.inner.lock();
        guard.clear();
        self.count.store(0, Ordering::Release);
        self.total_bytes.store(0, Ordering::Release);
    }

    pub fn recompute_stats(&self) {
        let guard = self.inner.lock();
        let count = guard.len();
        let total: u64 = guard.values()
            .map(|s| s.total_bytes)
            .fold(0u64, |a, b| a.saturating_add(b));
        self.count.store(count, Ordering::Release);
        self.total_bytes.store(total, Ordering::Release);
    }

    pub fn verify_consistency(&self) -> Result<(), ListConsistencyError> {
        let guard = self.inner.lock();
        let actual_count = guard.len();
        let actual_bytes: u64 = guard.values()
            .map(|s| s.total_bytes)
            .fold(0u64, |a, b| a.saturating_add(b));
        let tc = self.count.load(Ordering::Acquire);
        let tb = self.total_bytes.load(Ordering::Acquire);
        if tc != actual_count {
            return Err(ListConsistencyError::CountMismatch { tracked: tc, actual: actual_count });
        }
        if tb != actual_bytes {
            return Err(ListConsistencyError::BytesMismatch { tracked: tb, actual: actual_bytes });
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────
// Types auxiliaires
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct ListStats {
    pub count:              usize,
    pub total_bytes:        u64,
    pub oldest_created_at:  u64,
    pub newest_created_at:  u64,
    pub n_protected:        usize,
    pub n_readonly:         usize,
    pub n_orphan:           usize,
    pub max_blobs_in_one:   u64,
    pub max_bytes_in_one:   u64,
}

#[derive(Debug)]
pub enum ListConsistencyError {
    CountMismatch { tracked: usize, actual: usize },
    BytesMismatch { tracked: u64, actual: u64 },
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::{BlobId, DiskOffset};
    use super::super::snapshot::make_snapshot_name;

    fn make_snap(id: u64, bytes: u64, parent: Option<u64>) -> Snapshot {
        Snapshot {
            id: SnapshotId(id), epoch_id: EpochId(1),
            parent_id: parent.map(SnapshotId),
            root_blob: BlobId([0u8; 32]),
            created_at: 1000 + id, n_blobs: 5, total_bytes: bytes,
            flags: 0, blob_catalog_offset: DiskOffset(0), blob_catalog_size: 0,
            name: make_snapshot_name(b"test"),
        }
    }

    #[test]
    fn register_and_get() {
        let list = SnapshotList::new_const();
        list.register(make_snap(10, 4096, None)).unwrap();
        let got = list.get(SnapshotId(10)).unwrap();
        assert_eq!(got.total_bytes, 4096);
        assert_eq!(list.count(), 1);
    }

    #[test]
    fn remove_updates_stats() {
        let list = SnapshotList::new_const();
        list.register(make_snap(1, 1024, None)).unwrap();
        list.register(make_snap(2, 2048, Some(1))).unwrap();
        list.remove(SnapshotId(1)).unwrap();
        assert_eq!(list.count(), 1);
        assert_eq!(list.total_bytes(), 2048);
    }

    #[test]
    fn children_of_correct() {
        let list = SnapshotList::new_const();
        for i in 1..=4u64 {
            let parent = if i == 1 { None } else if i <= 3 { Some(1) } else { Some(2) };
            list.register(make_snap(i, 0, parent)).unwrap();
        }
        let children = list.children_of(SnapshotId(1)).unwrap();
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn duplicate_register_fails() {
        let list = SnapshotList::new_const();
        list.register(make_snap(5, 0, None)).unwrap();
        assert!(matches!(list.register(make_snap(5, 0, None)), Err(ExofsError::InvalidState)));
    }

    #[test]
    fn consistency_ok() {
        let list = SnapshotList::new_const();
        list.register(make_snap(20, 512, None)).unwrap();
        assert!(list.verify_consistency().is_ok());
    }

    #[test]
    fn all_refs_sorted() {
        let list = SnapshotList::new_const();
        for i in [3u64, 1, 2] { list.register(make_snap(i, 0, None)).unwrap(); }
        let refs = list.all_refs().unwrap();
        assert_eq!(refs[0].id.0, 1);
        assert_eq!(refs[2].id.0, 3);
    }
}
