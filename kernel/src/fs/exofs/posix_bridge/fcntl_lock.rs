//! fcntl_lock.rs — POSIX byte-range locking (F_SETLK / F_GETLK / F_SETLKW)
//!
//! Implements a POSIX-compatible byte-range lock manager on top of
//! ExoFS objects. Each object has its own lock queue indexed by its
//! `object_id`. Write/Write and Read/Write conflicts are detected.
//!
//! RECUR-01 / OOM-02 / ARITH-02 — ExofsError exclusively.

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_LOCKS_PER_OBJECT: usize = 256;
pub const MAX_OBJECTS_LOCKED: usize = 1024;
pub const LOCK_TABLE_MAGIC: u32 = 0x4C_4B_54_42; // "LKTB"
pub const LOCK_RANGE_MAX: u64 = u64::MAX / 2;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// POSIX lock type (equivalent to F_RDLCK / F_WRLCK / F_UNLCK).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    Read = 0,
    Write = 1,
    Unlock = 2,
}

/// Supported `fcntl` command.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FcntlCmd {
    GetLk = 0,  // F_GETLK
    SetLk = 1,  // F_SETLK
    SetLkW = 2, // F_SETLKW (non-blocking in this kernel — treated as SetLk)
}

/// Byte-range lock entry.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ByteRangeLock {
    pub object_id: u64,
    pub pid: u64,
    pub tid: u64,
    pub start: u64,
    pub length: u64,
    pub kind: LockKind,
    pub _pad: [u8; 7],
}

const _: () = assert!(core::mem::size_of::<ByteRangeLock>() == 48);

/// Result of an F_GETLK request.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct LockInfo {
    pub conflicting_pid: u64,
    pub conflicting_tid: u64,
    pub start: u64,
    pub length: u64,
    pub kind: u8,
    pub blocked: u8,
    pub _pad: [u8; 6],
}

const _: () = assert!(core::mem::size_of::<LockInfo>() == 40);

// ─────────────────────────────────────────────────────────────────────────────
// Internal table entry (by object_id)
// ─────────────────────────────────────────────────────────────────────────────

struct ObjectLockSlot {
    object_id: u64,
    locks: Vec<ByteRangeLock>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Global table — spinlock via UnsafeCell + AtomicU64
// ─────────────────────────────────────────────────────────────────────────────

use core::cell::UnsafeCell;

pub struct FcntlLockTable {
    slots: UnsafeCell<Vec<ObjectLockSlot>>,
    spinlock: AtomicU64,
    count: AtomicU64,
}

unsafe impl Sync for FcntlLockTable {}
unsafe impl Send for FcntlLockTable {}

pub static FCNTL_LOCK_TABLE: FcntlLockTable = FcntlLockTable::new_const();

impl FcntlLockTable {
    pub const fn new_const() -> Self {
        Self {
            slots: UnsafeCell::new(Vec::new()),
            spinlock: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    fn lock_acquire(&self) {
        while self
            .spinlock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    fn lock_release(&self) {
        self.spinlock.store(0, Ordering::Release);
    }

    // ─── Search or create slot for an object_id ───

    /// Finds slot index for `object_id`. RECUR-01: while.
    fn find_slot(slots: &[ObjectLockSlot], object_id: u64) -> Option<usize> {
        let mut i = 0usize;
        while i < slots.len() {
            if slots[i].object_id == object_id {
                return Some(i);
            }
            i = i.wrapping_add(1);
        }
        None
    }

    // ─── Test overlap between two ranges ───

    /// Returns true if [a_start, a_start+a_len) overlaps [b_start, b_start+b_len).
    /// ARITH-02: checked_add.
    fn overlaps(a_start: u64, a_len: u64, b_start: u64, b_len: u64) -> bool {
        let a_end = a_start.saturating_add(a_len);
        let b_end = b_start.saturating_add(b_len);
        a_start < b_end && b_start < a_end
    }

    /// Tests if a lock conflicts with existing locks.
    /// RECUR-01: while.
    fn has_conflict(existing: &[ByteRangeLock], candidate: &ByteRangeLock) -> Option<usize> {
        let mut i = 0usize;
        while i < existing.len() {
            let e = &existing[i];
            // Same pid/tid -> no internal conflict.
            if e.pid == candidate.pid && e.tid == candidate.tid {
                i = i.wrapping_add(1);
                continue;
            }
            let over = Self::overlaps(candidate.start, candidate.length, e.start, e.length);
            if over && (candidate.kind == LockKind::Write || e.kind == LockKind::Write) {
                return Some(i);
            }
            i = i.wrapping_add(1);
        }
        None
    }

    // ─── Main API ───

    /// Acquires a byte-range lock (F_SETLK / F_SETLKW).
    /// OOM-02: try_reserve. RECUR-01: while.
    pub fn acquire(&self, lock: ByteRangeLock) -> ExofsResult<()> {
        if lock.kind == LockKind::Unlock {
            return self.release(lock.object_id, lock.pid, lock.tid, lock.start, lock.length);
        }
        if lock.length == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        if lock.start > LOCK_RANGE_MAX {
            return Err(ExofsError::InvalidArgument);
        }

        self.lock_acquire();
        let result = self.acquire_inner(lock);
        self.lock_release();
        result
    }

    fn acquire_inner(&self, lock: ByteRangeLock) -> ExofsResult<()> {
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let slots = unsafe { &mut *self.slots.get() };

        if let Some(idx) = Self::find_slot(slots, lock.object_id) {
            if let Some(_) = Self::has_conflict(&slots[idx].locks, &lock) {
                return Err(ExofsError::PermissionDenied);
            }
            if slots[idx].locks.len() >= MAX_LOCKS_PER_OBJECT {
                return Err(ExofsError::QuotaExceeded);
            }
            slots[idx]
                .locks
                .try_reserve(1)
                .map_err(|_| ExofsError::NoMemory)?;
            slots[idx].locks.push(lock);
        } else {
            if slots.len() >= MAX_OBJECTS_LOCKED {
                return Err(ExofsError::QuotaExceeded);
            }
            slots.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            let mut v: Vec<ByteRangeLock> = Vec::new();
            v.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            v.push(lock);
            slots.push(ObjectLockSlot {
                object_id: lock.object_id,
                locks: v,
            });
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Releases locks of a pid/tid over a range of an object.
    /// RECUR-01: while.
    pub fn release(
        &self,
        object_id: u64,
        pid: u64,
        tid: u64,
        start: u64,
        length: u64,
    ) -> ExofsResult<()> {
        self.lock_acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let slots = unsafe { &mut *self.slots.get() };
        if let Some(idx) = Self::find_slot(slots, object_id) {
            let v = &mut slots[idx].locks;
            let mut i = 0usize;
            while i < v.len() {
                let l = &v[i];
                let matches_owner = l.pid == pid && l.tid == tid;
                let over = length == 0 || Self::overlaps(start, length, l.start, l.length);
                if matches_owner && over {
                    v.remove(i);
                    // do not increment i — alternative swap-remove
                } else {
                    i = i.wrapping_add(1);
                }
            }
            if slots[idx].locks.is_empty() {
                slots.remove(idx);
                self.count.fetch_sub(1, Ordering::Relaxed);
            }
        }
        self.lock_release();
        Ok(())
    }

    /// Releases ALL locks for a pid (upon process termination).
    /// RECUR-01: while.
    pub fn release_all_pid(&self, pid: u64) {
        self.lock_acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let slots = unsafe { &mut *self.slots.get() };
        let mut si = 0usize;
        while si < slots.len() {
            let v = &mut slots[si].locks;
            let mut li = 0usize;
            while li < v.len() {
                if v[li].pid == pid {
                    v.remove(li);
                } else {
                    li = li.wrapping_add(1);
                }
            }
            if slots[si].locks.is_empty() {
                slots.remove(si);
                self.count.fetch_sub(1, Ordering::Relaxed);
            } else {
                si = si.wrapping_add(1);
            }
        }
        self.lock_release();
    }

    /// F_GETLK: tests if a lock is possible, returns the conflict if present.
    pub fn test_lock(&self, candidate: &ByteRangeLock) -> ExofsResult<Option<LockInfo>> {
        self.lock_acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let slots = unsafe { &*self.slots.get() };
        let result = if let Some(idx) = Self::find_slot(slots, candidate.object_id) {
            match Self::has_conflict(&slots[idx].locks, candidate) {
                Some(ci) => {
                    let c = &slots[idx].locks[ci];
                    Ok(Some(LockInfo {
                        conflicting_pid: c.pid,
                        conflicting_tid: c.tid,
                        start: c.start,
                        length: c.length,
                        kind: c.kind as u8,
                        blocked: 1,
                        _pad: [0; 6],
                    }))
                }
                None => Ok(None),
            }
        } else {
            Ok(None)
        };
        self.lock_release();
        result
    }

    /// Clears all locks.
    pub fn clear(&self) {
        self.lock_acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let slots = unsafe { &mut *self.slots.get() };
        slots.clear();
        self.count.store(0, core::sync::atomic::Ordering::Relaxed);
        self.lock_release();
    }

    /// Returns the number of currently locked objects.
    pub fn locked_object_count(&self) -> usize {
        self.lock_acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let n = unsafe { (*self.slots.get()).len() };
        self.lock_release();
        n
    }

    /// Returns the number of active locks on an object.
    pub fn lock_count_for(&self, object_id: u64) -> usize {
        self.lock_acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let slots = unsafe { &*self.slots.get() };
        let n = if let Some(idx) = Self::find_slot(slots, object_id) {
            slots[idx].locks.len()
        } else {
            0
        };
        self.lock_release();
        n
    }

    /// Returns total number of active locks across all objects.
    pub fn total_lock_count(&self) -> usize {
        self.lock_acquire();
        // SAFETY: exclusive access guaranteed by atomic lock acquired beforehand.
        let slots = unsafe { &*self.slots.get() };
        let mut total = 0usize;
        let mut i = 0usize;
        while i < slots.len() {
            total = total.saturating_add(slots[i].locks.len());
            i = i.wrapping_add(1);
        }
        self.lock_release();
        total
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Ergonomically constructs a ByteRangeLock.
pub fn make_lock(
    object_id: u64,
    pid: u64,
    tid: u64,
    start: u64,
    length: u64,
    kind: LockKind,
) -> ByteRangeLock {
    ByteRangeLock {
        object_id,
        pid,
        tid,
        start,
        length,
        kind,
        _pad: [0; 7],
    }
}

/// Returns true if two ranges overlap.
/// ARITH-02: saturating_add.
pub fn ranges_overlap(a_start: u64, a_len: u64, b_start: u64, b_len: u64) -> bool {
    let a_end = a_start.saturating_add(a_len);
    let b_end = b_start.saturating_add(b_len);
    a_start < b_end && b_start < a_end
}

/// Returns total size of a union of two ranges (without holes).
/// ARITH-02: saturating_add/sub.
pub fn union_range_size(a_start: u64, a_len: u64, b_start: u64, b_len: u64) -> u64 {
    let a_end = a_start.saturating_add(a_len);
    let b_end = b_start.saturating_add(b_len);
    let start = a_start.min(b_start);
    let end = a_end.max(b_end);
    end.saturating_sub(start)
}

/// Validates lock parameters.
pub fn validate_lock(lock: &ByteRangeLock) -> ExofsResult<()> {
    if lock.length == 0 {
        return Err(ExofsError::InvalidArgument);
    }
    if lock.start > LOCK_RANGE_MAX {
        return Err(ExofsError::InvalidArgument);
    }
    if lock.start.checked_add(lock.length).is_none() {
        return Err(ExofsError::InvalidArgument);
    }
    match lock.kind {
        LockKind::Read | LockKind::Write | LockKind::Unlock => Ok(()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn tbl() -> FcntlLockTable {
        FcntlLockTable::new_const()
    }

    fn mk(oid: u64, pid: u64, start: u64, len: u64, k: LockKind) -> ByteRangeLock {
        make_lock(oid, pid, 0, start, len, k)
    }

    #[test]
    fn test_lock_size() {
        assert_eq!(core::mem::size_of::<ByteRangeLock>(), 48);
    }

    #[test]
    fn test_lock_info_size() {
        assert_eq!(core::mem::size_of::<LockInfo>(), 40);
    }

    #[test]
    fn test_acquire_read_ok() {
        let t = tbl();
        let l = mk(1, 100, 0, 512, LockKind::Read);
        assert!(t.acquire(l).is_ok());
    }

    #[test]
    fn test_two_reads_no_conflict() {
        let t = tbl();
        assert!(t.acquire(mk(1, 100, 0, 512, LockKind::Read)).is_ok());
        assert!(t.acquire(mk(1, 101, 0, 512, LockKind::Read)).is_ok());
    }

    #[test]
    fn test_read_write_conflict() {
        let t = tbl();
        t.acquire(mk(1, 100, 0, 512, LockKind::Read)).test_unwrap();
        assert!(t.acquire(mk(1, 200, 0, 512, LockKind::Write)).is_err());
    }

    #[test]
    fn test_write_write_conflict() {
        let t = tbl();
        t.acquire(mk(1, 100, 0, 512, LockKind::Write)).test_unwrap();
        assert!(t.acquire(mk(1, 200, 0, 512, LockKind::Write)).is_err());
    }

    #[test]
    fn test_no_overlap_no_conflict() {
        let t = tbl();
        t.acquire(mk(1, 100, 0, 256, LockKind::Write)).test_unwrap();
        assert!(t.acquire(mk(1, 200, 256, 256, LockKind::Write)).is_ok());
    }

    #[test]
    fn test_release() {
        let t = tbl();
        t.acquire(mk(1, 100, 0, 512, LockKind::Write)).test_unwrap();
        t.release(1, 100, 0, 0, 0).test_unwrap();
        assert_eq!(t.lock_count_for(1), 0);
    }

    #[test]
    fn test_release_all_pid() {
        let t = tbl();
        t.acquire(mk(1, 100, 0, 256, LockKind::Write)).test_unwrap();
        t.acquire(mk(2, 100, 0, 256, LockKind::Write)).test_unwrap();
        t.release_all_pid(100);
        assert_eq!(t.locked_object_count(), 0);
    }

    #[test]
    fn test_test_lock_conflict() {
        let t = tbl();
        t.acquire(mk(1, 100, 0, 512, LockKind::Write)).test_unwrap();
        let candidate = mk(1, 200, 0, 512, LockKind::Write);
        let info = t.test_lock(&candidate).test_unwrap();
        assert!(info.is_some());
        assert_eq!(info.test_unwrap().conflicting_pid, 100);
    }

    #[test]
    fn test_validate_lock_zero_len() {
        let l = make_lock(1, 1, 0, 0, 0, LockKind::Read);
        assert!(validate_lock(&l).is_err());
    }

    #[test]
    fn test_ranges_overlap() {
        assert!(ranges_overlap(0, 10, 5, 10));
        assert!(!ranges_overlap(0, 5, 5, 5));
    }

    #[test]
    fn test_union_range_size() {
        assert_eq!(union_range_size(0, 10, 5, 10), 15);
    }

    #[test]
    fn test_same_pid_no_conflict() {
        let t = tbl();
        t.acquire(mk(1, 100, 0, 512, LockKind::Write)).test_unwrap();
        // Same pid -> no conflict (POSIX rule)
        assert!(t.acquire(mk(1, 100, 0, 512, LockKind::Write)).is_ok());
    }
}
