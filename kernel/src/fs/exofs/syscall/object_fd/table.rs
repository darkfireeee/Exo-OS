//! Synchronized ExoFS open-object FD table (`ObjectFdTable`, `OBJECT_TABLE`, `OBJECT_LIFECYCLE_LOCK`).
//!
//! Extracted from `exofs/syscall/object_fd/mod.rs` so `mod.rs` remains a pure wiring facade.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::entry::{current_owner_pid, owner_matches, ObjectFdEntry, FD_RESERVED, MAX_FDS};
use super::flags as open_flags;
use super::table_inner::ObjectFdTableInner;
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::SpinLock;

pub static OBJECT_LIFECYCLE_LOCK: SpinLock<()> = SpinLock::new(());

pub struct ObjectFdTable {
    inner: UnsafeCell<ObjectFdTableInner>,
    lock: AtomicU64,
    next_hint: AtomicU32,
    opens: AtomicU64,
    closes: AtomicU64,
    errors: AtomicU64,
}

unsafe impl Sync for ObjectFdTable {}
unsafe impl Send for ObjectFdTable {}

impl ObjectFdTable {
    pub const fn new_const() -> Self {
        Self {
            inner: UnsafeCell::new(ObjectFdTableInner::new()),
            lock: AtomicU64::new(0),
            next_hint: AtomicU32::new(FD_RESERVED),
            opens: AtomicU64::new(0),
            closes: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }

    fn acquire(&self) {
        while self
            .lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    fn release(&self) {
        self.lock.store(0, Ordering::Release);
    }

    pub fn open(
        &self,
        blob_id: BlobId,
        flags: u32,
        size: u64,
        epoch_id: u64,
        owner_uid: u64,
    ) -> ExofsResult<u32> {
        if !open_flags::validate(flags) {
            self.errors.fetch_add(1, Ordering::Relaxed);
            return Err(ExofsError::InvalidArgument);
        }
        let effective_owner = if owner_uid == 0 {
            current_owner_pid()
        } else {
            owner_uid
        };
        self.acquire();
        let r = unsafe { &mut *self.inner.get() }.open_slot(
            blob_id,
            flags,
            size,
            epoch_id,
            effective_owner,
            self.next_hint.load(Ordering::Relaxed),
        );
        self.release();
        match &r {
            Ok(_) => {
                self.opens.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        r
    }

    pub fn close(&self, fd: u32) -> bool {
        self.acquire();
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let found = match inner.get_entry(fd) {
            Ok(entry) if owner_matches(entry, owner) => inner.close_slot(fd),
            _ => false,
        };
        self.release();
        if found {
            self.closes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    pub fn close_for_cleanup(&self, fd: u32) -> Option<BlobId> {
        self.acquire();
        let inner = unsafe { &mut *self.inner.get() };
        let blob_id = inner.get_entry(fd).ok().map(|e| e.blob_id);
        let found = if blob_id.is_some() {
            inner.close_slot(fd)
        } else {
            false
        };
        self.release();
        if found {
            self.closes.fetch_add(1, Ordering::Relaxed);
            blob_id
        } else {
            self.errors.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    pub fn get(&self, fd: u32) -> ExofsResult<ObjectFdEntry> {
        self.acquire();
        let r = unsafe { &*self.inner.get() }.get_entry(fd);
        self.release();
        match r {
            Ok(entry) => {
                if !owner_matches(entry, current_owner_pid()) {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                    return Err(ExofsError::PermissionDenied);
                }
                Ok(entry)
            }
            Err(err) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                Err(err)
            }
        }
    }

    pub fn blob_id_of(&self, fd: u32) -> ExofsResult<BlobId> {
        self.get(fd).map(|e| e.blob_id)
    }

    pub fn set_cursor(&self, fd: u32, cursor: u64) -> ExofsResult<()> {
        self.get(fd)?;
        self.acquire();
        let r = unsafe { &mut *self.inner.get() }.set_cursor(fd, cursor);
        self.release();
        r
    }

    pub fn set_size(&self, fd: u32, size: u64) -> ExofsResult<()> {
        self.get(fd)?;
        self.acquire();
        let r = unsafe { &mut *self.inner.get() }.set_size(fd, size);
        self.release();
        r
    }

    pub fn advance_cursor(&self, fd: u32, n: u64) -> ExofsResult<u64> {
        self.get(fd)?;
        self.acquire();
        let inner = unsafe { &mut *self.inner.get() };
        let idx = inner.canonical_slot_of_fd(fd);
        match idx {
            Err(e) => {
                self.release();
                Err(e)
            }
            Ok(i) => {
                inner.slots[i].advance_cursor(n);
                let new_cursor = inner.slots[i].cursor;
                self.release();
                Ok(new_cursor)
            }
        }
    }

    pub fn dup(&self, fd: u32) -> ExofsResult<u32> {
        self.acquire();
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(fd) {
            Ok(entry) if owner_matches(entry, owner) => inner.dup_fd(fd),
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    pub fn dup_checked_for_owner(&self, fd: u32, target_owner: u64) -> ExofsResult<u32> {
        self.acquire();
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(fd) {
            Ok(entry) if owner_matches(entry, owner) => inner.dup_fd_with_owner(fd, target_owner),
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    pub fn dup_transferred(&self, fd: u32, target_owner: u64) -> ExofsResult<u32> {
        self.acquire();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(fd) {
            Ok(entry) if entry.owner_uid == 0 => inner.dup_fd_with_owner(fd, target_owner),
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    pub fn dup2(&self, oldfd: u32, newfd: u32) -> ExofsResult<u32> {
        self.acquire();
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(oldfd) {
            Ok(entry) if owner_matches(entry, owner) => match inner.get_entry(newfd) {
                Ok(existing) if !owner_matches(existing, owner) => {
                    Err(ExofsError::PermissionDenied)
                }
                _ => inner.dup2_fd(oldfd, newfd),
            },
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    pub fn dup_from(&self, fd: u32, min_fd: u32) -> ExofsResult<u32> {
        self.acquire();
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(fd) {
            Ok(entry) if owner_matches(entry, owner) => inner.dup_from_fd(fd, min_fd),
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    pub fn set_status_flags(&self, fd: u32, flags: u32) -> ExofsResult<u32> {
        self.acquire();
        let owner = current_owner_pid();
        let inner = unsafe { &mut *self.inner.get() };
        let r = match inner.get_entry(fd) {
            Ok(entry) if owner_matches(entry, owner) => inner.set_status_flags(fd, flags),
            Ok(_) => Err(ExofsError::PermissionDenied),
            Err(err) => Err(err),
        };
        self.release();
        r
    }

    pub fn open_count(&self) -> u32 {
        self.acquire();
        let n = unsafe { &*self.inner.get() }.open_count();
        self.release();
        n
    }

    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.opens.load(Ordering::Relaxed),
            self.closes.load(Ordering::Relaxed),
            self.errors.load(Ordering::Relaxed),
        )
    }

    pub fn check_readable(&self, fd: u32) -> ExofsResult<()> {
        let entry = self.get(fd)?;
        if !entry.can_read() {
            return Err(ExofsError::PermissionDenied);
        }
        Ok(())
    }

    pub fn check_writable(&self, fd: u32) -> ExofsResult<()> {
        let entry = self.get(fd)?;
        if !entry.can_write() {
            return Err(ExofsError::PermissionDenied);
        }
        Ok(())
    }

    pub fn reset_all(&self) {
        self.acquire();
        let inner = unsafe { &mut *self.inner.get() };
        let mut i = 0usize;
        while i < MAX_FDS {
            inner.slots[i] = ObjectFdEntry::empty();
            i = i.wrapping_add(1);
        }
        inner.open_count = 0;
        self.release();
        self.opens.store(0, Ordering::Relaxed);
        self.closes.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
    }

    #[inline]
    pub fn lock(&self) -> Result<&Self, ExofsError> {
        Ok(self)
    }

    pub fn open_count_for(&self, id: &BlobId) -> usize {
        self.acquire();
        let inner = unsafe { &*self.inner.get() };
        let mut count = 0usize;
        let mut i = 0usize;
        while i < MAX_FDS {
            if !inner.slots[i].is_free() && inner.slots[i].blob_id == *id {
                count = count.saturating_add(1);
            }
            i = i.wrapping_add(1);
        }
        self.release();
        count
    }

    pub fn readable_count_for(&self, id: &BlobId) -> usize {
        self.acquire();
        let inner = unsafe { &*self.inner.get() };
        let mut count = 0usize;
        let mut i = 0usize;
        while i < MAX_FDS {
            let entry = inner.slots[i];
            if !entry.is_free() && entry.blob_id == *id && entry.can_read() {
                count = count.saturating_add(1);
            }
            i = i.wrapping_add(1);
        }
        self.release();
        count
    }

    pub fn entry_for_blob(&self, id: &BlobId) -> Option<ObjectFdEntry> {
        self.acquire();
        let inner = unsafe { &*self.inner.get() };
        let mut found = None;
        let mut i = 0usize;
        while i < MAX_FDS {
            let entry = inner.slots[i];
            if !entry.is_free() && entry.blob_id == *id {
                found = Some(entry);
                break;
            }
            i = i.wrapping_add(1);
        }
        self.release();
        found
    }
}

pub static OBJECT_TABLE: ObjectFdTable = ObjectFdTable::new_const();
