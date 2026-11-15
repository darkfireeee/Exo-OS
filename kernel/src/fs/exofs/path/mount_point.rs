//! mount_point.rs — ExoFS mount point table
//!
//! Applied rules:
//!  - OOM-02   : systematic try_reserve
//!  - ONDISK-03: no AtomicU64 in repr(C) structs
//!  - ARITH-02 : checked arithmetic

extern crate alloc;

use crate::fs::exofs::core::{ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::path::path_component::NAME_MAX;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of simultaneous mount points.
pub const MOUNT_TABLE_MAX: usize = 64;

/// Mount flags.
pub const MOUNT_FLAG_READONLY: u32 = 0x0001;
pub const MOUNT_FLAG_NOEXEC: u32 = 0x0002;
pub const MOUNT_FLAG_NOSUID: u32 = 0x0004;
pub const MOUNT_FLAG_BIND: u32 = 0x0008;

// ─────────────────────────────────────────────────────────────────────────────
// MountPoint
// ─────────────────────────────────────────────────────────────────────────────

/// A mount point: host directory -> mounted filesystem.
#[derive(Clone)]
pub struct MountPoint {
    /// OID of the directory on which the mount is performed.
    pub dir_oid: ObjectId,
    /// Name of the mount point (often the last component of the path).
    pub name: [u8; NAME_MAX + 1],
    /// Valid length of the name.
    pub name_len: u16,
    /// Root OID of the mounted filesystem.
    pub mounted_oid: ObjectId,
    /// Mount flags (MOUNT_FLAG_*).
    pub flags: u32,
    /// Unique identifier of this mount.
    pub mount_id: u32,
    /// Creation tick.
    pub created_tick: u64,
}

impl MountPoint {
    /// Constructor.
    pub fn new(
        dir_oid: ObjectId,
        name: &[u8],
        mounted_oid: ObjectId,
        flags: u32,
        mount_id: u32,
    ) -> ExofsResult<Self> {
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(ExofsError::InvalidPathComponent);
        }
        let mut name_buf = [0u8; NAME_MAX + 1];
        name_buf[..name.len()].copy_from_slice(name);
        Ok(MountPoint {
            dir_oid,
            name: name_buf,
            name_len: name.len() as u16,
            mounted_oid,
            flags,
            mount_id,
            created_tick: crate::arch::time::read_ticks(),
        })
    }

    /// Mount point name.
    #[inline]
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }

    /// `true` if this mount point is read-only.
    #[inline]
    pub fn is_readonly(&self) -> bool {
        self.flags & MOUNT_FLAG_READONLY != 0
    }

    /// `true` if executables are prohibited.
    #[inline]
    pub fn is_noexec(&self) -> bool {
        self.flags & MOUNT_FLAG_NOEXEC != 0
    }

    /// `true` if setuid is prohibited.
    #[inline]
    pub fn is_nosuid(&self) -> bool {
        self.flags & MOUNT_FLAG_NOSUID != 0
    }
}

// `Default` for `const` array initialization.
impl Default for MountPoint {
    fn default() -> Self {
        MountPoint {
            dir_oid: ObjectId([0u8; 32]),
            name: [0u8; NAME_MAX + 1],
            name_len: 0,
            mounted_oid: ObjectId([0u8; 32]),
            flags: 0,
            mount_id: 0,
            created_tick: 0,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MountTableInner
// ─────────────────────────────────────────────────────────────────────────────

const EMPTY_MOUNT: MountPoint = MountPoint {
    dir_oid: ObjectId([0u8; 32]),
    name: [0u8; NAME_MAX + 1],
    name_len: 0,
    mounted_oid: ObjectId([0u8; 32]),
    flags: 0,
    mount_id: 0,
    created_tick: 0,
};

struct MountTableInner {
    entries: [MountPoint; MOUNT_TABLE_MAX],
    count: usize,
    next_id: u32,
}

impl MountTableInner {
    const fn new() -> Self {
        // We cannot call `Default::default()` in const fn yet,
        // so we initialize manually.
        MountTableInner {
            entries: [EMPTY_MOUNT; MOUNT_TABLE_MAX],
            count: 0,
            next_id: 1,
        }
    }

    /// Finds a free slot (name_len == 0).
    fn free_slot(&self) -> Option<usize> {
        for i in 0..MOUNT_TABLE_MAX {
            if self.entries[i].name_len == 0 {
                return Some(i);
            }
        }
        None
    }

    /// Finds the mount point whose dir_oid matches.
    fn find_by_dir(&self, oid: &ObjectId) -> Option<usize> {
        for i in 0..MOUNT_TABLE_MAX {
            if self.entries[i].name_len != 0 && self.entries[i].dir_oid.0 == oid.0 {
                return Some(i);
            }
        }
        None
    }

    /// Finds by mount_id.
    fn find_by_id(&self, id: u32) -> Option<usize> {
        for i in 0..MOUNT_TABLE_MAX {
            if self.entries[i].name_len != 0 && self.entries[i].mount_id == id {
                return Some(i);
            }
        }
        None
    }

    fn insert(&mut self, mp: MountPoint) -> ExofsResult<u32> {
        if self.count >= MOUNT_TABLE_MAX {
            return Err(ExofsError::NoSpace);
        }
        let slot = self.free_slot().ok_or(ExofsError::NoSpace)?;
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        let mut mp = mp;
        mp.mount_id = id;
        self.entries[slot] = mp;
        self.count = self
            .count
            .checked_add(1)
            .ok_or(ExofsError::OffsetOverflow)?;
        Ok(id)
    }

    fn remove_by_dir(&mut self, oid: &ObjectId) -> ExofsResult<()> {
        let slot = self.find_by_dir(oid).ok_or(ExofsError::ObjectNotFound)?;
        self.entries[slot] = EMPTY_MOUNT;
        self.count = self.count.saturating_sub(1);
        Ok(())
    }

    fn remove_by_id(&mut self, id: u32) -> ExofsResult<()> {
        let slot = self.find_by_id(id).ok_or(ExofsError::ObjectNotFound)?;
        self.entries[slot] = EMPTY_MOUNT;
        self.count = self.count.saturating_sub(1);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MountTable (public, thread-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// Global mount point table, protected by SpinLock.
pub struct MountTable {
    inner: SpinLock<MountTableInner>,
}

impl MountTable {
    /// `const` constructor for static initialization.
    pub const fn new_const() -> Self {
        MountTable {
            inner: SpinLock::new(MountTableInner::new()),
        }
    }

    /// Registers a new mount point.
    ///
    /// Returns the allocated `mount_id`.
    pub fn register(
        &self,
        dir_oid: ObjectId,
        name: &[u8],
        mounted_oid: ObjectId,
        flags: u32,
    ) -> ExofsResult<u32> {
        let mp = MountPoint::new(dir_oid, name, mounted_oid, flags, 0)?;
        let mut guard = self.inner.lock();
        guard.insert(mp)
    }

    /// Removes the mount point on `dir_oid`.
    pub fn unregister_by_dir(&self, dir_oid: &ObjectId) -> ExofsResult<()> {
        let mut guard = self.inner.lock();
        guard.remove_by_dir(dir_oid)
    }

    /// Removes the mount point by its identifier.
    pub fn unregister_by_id(&self, mount_id: u32) -> ExofsResult<()> {
        let mut guard = self.inner.lock();
        guard.remove_by_id(mount_id)
    }

    /// Looks up a mount point covering `dir_oid`.
    ///
    /// Returns the mounted OID if found.
    pub fn lookup_mount(&self, dir_oid: &ObjectId) -> Option<ObjectId> {
        let guard = self.inner.lock();
        guard
            .find_by_dir(dir_oid)
            .map(|i| guard.entries[i].mounted_oid.clone())
    }

    /// Returns a cloned copy of the entry for `dir_oid`.
    pub fn get_entry(&self, dir_oid: &ObjectId) -> Option<MountPoint> {
        let guard = self.inner.lock();
        guard.find_by_dir(dir_oid).map(|i| guard.entries[i].clone())
    }

    /// Number of active mounts.
    pub fn count(&self) -> usize {
        self.inner.lock().count
    }

    /// Checks whether an OID is covered by a mount (read-only).
    pub fn is_readonly(&self, dir_oid: &ObjectId) -> bool {
        let guard = self.inner.lock();
        match guard.find_by_dir(dir_oid) {
            None => false,
            Some(i) => guard.entries[i].is_readonly(),
        }
    }

    /// Iterates over all active mount points.
    ///
    /// Calls `f(mp)` for each valid entry.
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&MountPoint),
    {
        let guard = self.inner.lock();
        for i in 0..MOUNT_TABLE_MAX {
            if guard.entries[i].name_len != 0 {
                f(&guard.entries[i]);
            }
        }
    }

    /// Flushes the table to zero (shutdown).
    pub fn flush(&self) {
        let mut guard = self.inner.lock();
        for i in 0..MOUNT_TABLE_MAX {
            guard.entries[i] = EMPTY_MOUNT;
        }
        guard.count = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Global singleton
// ─────────────────────────────────────────────────────────────────────────────

/// Global mount point table.
pub static MOUNT_TABLE: MountTable = MountTable::new_const();

// ─────────────────────────────────────────────────────────────────────────────
// Convenience functions
// ─────────────────────────────────────────────────────────────────────────────

/// Registers a mount point in the global table.
#[inline]
pub fn register_mount(
    dir_oid: ObjectId,
    name: &[u8],
    mounted_oid: ObjectId,
    flags: u32,
) -> ExofsResult<u32> {
    MOUNT_TABLE.register(dir_oid, name, mounted_oid, flags)
}

/// Removes a mount by its directory.
#[inline]
pub fn unregister_mount_by_dir(dir_oid: &ObjectId) -> ExofsResult<()> {
    MOUNT_TABLE.unregister_by_dir(dir_oid)
}

/// Checks whether a directory is a mount point.
#[inline]
pub fn lookup_mount(dir_oid: &ObjectId) -> Option<ObjectId> {
    MOUNT_TABLE.lookup_mount(dir_oid)
}

/// `true` if `dir_oid` is a mount point (regardless of flags).
#[inline]
pub fn is_mount_point(dir_oid: &ObjectId) -> bool {
    MOUNT_TABLE.lookup_mount(dir_oid).is_some()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn oid(b: u8) -> ObjectId {
        let mut a = [0u8; 32];
        a[0] = b;
        ObjectId(a)
    }

    #[test]
    fn test_register_lookup() {
        let tbl = MountTable::new_const();
        tbl.register(oid(1), b"mnt", oid(99), 0).test_unwrap();
        let r = tbl.lookup_mount(&oid(1)).test_unwrap();
        assert_eq!(r.0[0], 99);
    }

    #[test]
    fn test_unregister() {
        let tbl = MountTable::new_const();
        tbl.register(oid(2), b"mnt2", oid(88), 0).test_unwrap();
        tbl.unregister_by_dir(&oid(2)).test_unwrap();
        assert!(tbl.lookup_mount(&oid(2)).is_none());
    }

    #[test]
    fn test_readonly_flag() {
        let tbl = MountTable::new_const();
        tbl.register(oid(3), b"ro", oid(77), MOUNT_FLAG_READONLY)
            .test_unwrap();
        assert!(tbl.is_readonly(&oid(3)));
    }

    #[test]
    fn test_table_full() {
        let tbl = MountTable::new_const();
        for i in 0u8..64 {
            tbl.register(oid(i), b"x", oid(i.wrapping_add(100)), 0)
                .test_unwrap();
        }
        let extra = tbl.register(oid(200), b"z", oid(201), 0);
        assert!(extra.is_err());
    }

    #[test]
    fn test_count() {
        let tbl = MountTable::new_const();
        tbl.register(oid(5), b"a", oid(50), 0).test_unwrap();
        tbl.register(oid(6), b"b", oid(60), 0).test_unwrap();
        assert_eq!(tbl.count(), 2);
    }

    #[test]
    fn test_flush() {
        let tbl = MountTable::new_const();
        tbl.register(oid(7), b"c", oid(70), 0).test_unwrap();
        tbl.flush();
        assert_eq!(tbl.count(), 0);
    }
}
