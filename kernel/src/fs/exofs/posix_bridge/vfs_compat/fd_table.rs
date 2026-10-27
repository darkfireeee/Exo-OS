#![allow(unused_imports, dead_code)]
use alloc::collections::btree_map::Entry;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::posix_bridge::inode_emulation::{
    decode_inode_entry, encode_inode_entry, inode_flags, InodeEntry, ObjectIno, INODE_EMULATION,
};
use crate::fs::exofs::syscall::object_store;
use crate::fs::exofs::syscall::object_write::{persist_cached_blob_if_disk, reject_if_immutable};
use crate::fs::exofs::syscall::quota_query::{check_quota, quota_add_usage, quota_sub_usage};

use super::*;

// ─────────────────────────────────────────────────────────────────────────────
// Table des descripteurs ouverts
// ─────────────────────────────────────────────────────────────────────────────

pub struct OpenFdTable {
    pub fds: UnsafeCell<[Option<VfsFd>; VFS_OPEN_MAX]>,
    pub fd_count: AtomicUsize,
    pub spinlock: AtomicU64,
    pub next_fd: AtomicU64,
}

unsafe impl Sync for OpenFdTable {}
unsafe impl Send for OpenFdTable {}

impl OpenFdTable {
    pub const fn new() -> Self {
        Self {
            fds: UnsafeCell::new([None; VFS_OPEN_MAX]),
            fd_count: AtomicUsize::new(0),
            spinlock: AtomicU64::new(0),
            next_fd: AtomicU64::new(3), // 0=stdin,1=stdout,2=stderr
        }
    }

    pub fn lock_acquire(&self) {
        while self
            .spinlock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    pub fn lock_release(&self) {
        self.spinlock.store(0, Ordering::Release);
    }

    pub fn open_fd(&self, ino: ObjectIno, flags: u32, pid: u32) -> ExofsResult<u64> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &mut *self.fds.get() };
        if self.fd_count.load(Ordering::Relaxed) >= VFS_OPEN_MAX {
            self.lock_release();
            return Err(ExofsError::QuotaExceeded);
        }
        let start = self.next_fd.load(Ordering::Relaxed) % VFS_OPEN_MAX as u64;
        let mut slot_idx = None;
        let mut offset = 0u64;
        while offset < VFS_OPEN_MAX as u64 {
            let idx = ((start + offset) % VFS_OPEN_MAX as u64) as usize;
            if fds[idx].is_none() {
                slot_idx = Some(idx);
                break;
            }
            offset = offset.wrapping_add(1);
        }
        let Some(slot_idx) = slot_idx else {
            self.lock_release();
            return Err(ExofsError::QuotaExceeded);
        };
        let fd = (slot_idx as u64).saturating_add(3);
        self.next_fd
            .store(((slot_idx + 1) % VFS_OPEN_MAX) as u64, Ordering::Relaxed);
        let entry = VfsFd {
            fd,
            ino,
            flags,
            offset: 0,
            pid,
            active: true,
        };
        fds[slot_idx] = Some(entry);
        self.fd_count.fetch_add(1, Ordering::Relaxed);
        self.lock_release();
        Ok(fd)
    }

    pub fn close_fd(&self, fd: u64) -> ExofsResult<()> {
        let caller = current_vfs_owner_pid();
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &mut *self.fds.get() };
        let mut found = false;
        let mut denied = false;
        let mut i = 0usize;
        while i < fds.len() {
            if let Some(entry) = fds[i] {
                if entry.fd == fd && entry.active {
                    if !vfs_owner_matches(entry, caller) {
                        denied = true;
                        break;
                    }
                    fds[i] = None;
                    self.fd_count.fetch_sub(1, Ordering::Relaxed);
                    found = true;
                    break;
                }
            }
            i = i.wrapping_add(1);
        }
        self.lock_release();
        if found {
            Ok(())
        } else if denied {
            Err(ExofsError::PermissionDenied)
        } else {
            Err(ExofsError::ObjectNotFound)
        }
    }

    pub fn get_fd(&self, fd: u64) -> Option<VfsFd> {
        let caller = current_vfs_owner_pid();
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &*self.fds.get() };
        let mut r = None;
        let mut i = 0usize;
        while i < fds.len() {
            if let Some(entry) = fds[i] {
                if entry.fd == fd && entry.active && vfs_owner_matches(entry, caller) {
                    r = Some(entry);
                    break;
                }
            }
            i = i.wrapping_add(1);
        }
        self.lock_release();
        r
    }

    pub fn update_offset(&self, fd: u64, new_offset: u64) {
        let caller = current_vfs_owner_pid();
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &mut *self.fds.get() };
        let mut i = 0usize;
        while i < fds.len() {
            if let Some(mut entry) = fds[i] {
                if entry.fd == fd && entry.active && vfs_owner_matches(entry, caller) {
                    entry.offset = new_offset;
                    fds[i] = Some(entry);
                    break;
                }
            }
            i = i.wrapping_add(1);
        }
        self.lock_release();
    }

    pub fn close_all_pid(&self, pid: u32) {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &mut *self.fds.get() };
        let mut i = 0usize;
        while i < fds.len() {
            if let Some(entry) = fds[i] {
                if entry.pid == pid && entry.active {
                    fds[i] = None;
                    self.fd_count.fetch_sub(1, Ordering::Relaxed);
                }
            }
            i = i.wrapping_add(1);
        }
        self.lock_release();
    }

    pub fn active_count(&self) -> usize {
        self.fd_count.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn reset_all(&self) {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &mut *self.fds.get() };
        let mut i = 0usize;
        while i < fds.len() {
            fds[i] = None;
            i = i.wrapping_add(1);
        }
        self.fd_count.store(0, Ordering::Release);
        self.next_fd.store(3, Ordering::Release);
        self.lock_release();
    }
}

pub static FD_TABLE: OpenFdTable = OpenFdTable::new();

#[inline]
pub fn current_vfs_owner_pid() -> u32 {
    #[cfg(target_os = "none")]
    {
        crate::syscall::fast_path::syscall_current_pid()
    }
    #[cfg(not(target_os = "none"))]
    {
        0
    }
}

#[inline]
pub fn vfs_owner_matches(entry: VfsFd, caller: u32) -> bool {
    #[cfg(not(target_os = "none"))]
    {
        caller == 0 || entry.pid == 0 || entry.pid == caller
    }
    #[cfg(target_os = "none")]
    {
        // En mode bare-metal : PID 1 (init) est privilégié, sinon vérification stricte
        caller == 1 || (entry.pid != 0 && entry.pid == caller)
    }
}

