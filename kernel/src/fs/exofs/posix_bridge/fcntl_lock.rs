//! fcntl_lock — verrouillage byte-range ExoFS à granularité kernel (no_std).

use alloc::collections::BTreeMap;
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::FsError;

/// Type de verrou.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    Read  = 0,
    Write = 1,
}

/// Entrée de verrou byte-range.
#[derive(Clone, Copy, Debug)]
pub struct ByteRangeLock {
    pub object_id: u64,
    pub pid:       u64,
    pub start:     u64,
    pub length:    u64,
    pub kind:      LockKind,
}

pub static FCNTL_LOCK_TABLE: FcntlLockTable = FcntlLockTable::new_const();

pub struct FcntlLockTable {
    locks: SpinLock<BTreeMap<u64, alloc::vec::Vec<ByteRangeLock>>>,
}

impl FcntlLockTable {
    pub const fn new_const() -> Self {
        Self { locks: SpinLock::new(BTreeMap::new()) }
    }

    /// Essaie d'acquérir un verrou byte-range.
    pub fn acquire(&self, lock: ByteRangeLock) -> Result<(), FsError> {
        let mut table = self.locks.lock();
        let v = if let Some(v) = table.get_mut(&lock.object_id) {
            v
        } else {
            table.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
            table.insert(lock.object_id, alloc::vec::Vec::new());
            table.get_mut(&lock.object_id).unwrap()
        };
        // Détection de conflit.
        for existing in v.iter() {
            let overlap = lock.start < existing.start + existing.length
                       && existing.start < lock.start + lock.length;
            if overlap && (lock.kind == LockKind::Write || existing.kind == LockKind::Write) {
                return Err(FsError::Busy);
            }
        }
        v.try_reserve(1).map_err(|_| FsError::OutOfMemory)?;
        v.push(lock);
        Ok(())
    }

    /// Libère tous les verrous d'un pid sur un objet.
    pub fn release(&self, object_id: u64, pid: u64) {
        let mut table = self.locks.lock();
        if let Some(v) = table.get_mut(&object_id) {
            v.retain(|l| l.pid != pid);
            if v.is_empty() { table.remove(&object_id); }
        }
    }
}
