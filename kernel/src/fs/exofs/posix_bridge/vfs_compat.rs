//! vfs_compat.rs — Couche de compatibilité VFS pour ExoFS
//!
//! Expose les opérations fichier/répertoire POSIX standard (lookup, open,
//! read, write, getattr, readdir, mkdir, unlink, rename) en s'appuyant sur
//! la table d'inodes émulés et le registre de blobs ExoFS.
//!
//! RECUR-01 / OOM-02 / ARITH-02 — ExofsError exclusivement.

use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::posix_bridge::inode_emulation::{
    inode_flags, InodeEntry, ObjectIno, INODE_EMULATION,
};
use crate::fs::exofs::syscall::object_store;
use alloc::collections::btree_map::Entry;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const VFS_ROOT_INO: ObjectIno = 1;
pub const VFS_NAME_MAX: usize = 255;
pub const VFS_PATH_MAX: usize = 4096;
pub const VFS_OPEN_MAX: usize = 1024;
pub const VFS_READDIR_BATCH: usize = 64;
/// Magic propre à la couche namespace/VFS.
/// Il est volontairement distinct du magic superblock ExoFS (`0x4558_4F46`).
pub const VFS_MAGIC: u32 = 0x5654_4654; // "VTFT"
pub const VFS_VERSION: u8 = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Modes et types de fichiers
// ─────────────────────────────────────────────────────────────────────────────

pub mod file_mode {
    pub const S_IFMT: u32 = 0o170000;
    pub const S_IFREG: u32 = 0o100000;
    pub const S_IFDIR: u32 = 0o040000;
    pub const S_IFLNK: u32 = 0o120000;
    pub const S_IRUSR: u32 = 0o000400;
    pub const S_IWUSR: u32 = 0o000200;
    pub const S_IXUSR: u32 = 0o000100;
    pub const S_IRGRP: u32 = 0o000040;
    pub const S_IWGRP: u32 = 0o000020;
    pub const S_IXGRP: u32 = 0o000010;
    pub const S_IROTH: u32 = 0o000004;
    pub const S_IWOTH: u32 = 0o000002;
    pub const S_IXOTH: u32 = 0o000001;
    pub const DEFAULT_DIR: u32 = S_IFDIR | S_IRUSR | S_IWUSR | S_IXUSR;
    pub const DEFAULT_FILE: u32 = S_IFREG | S_IRUSR | S_IWUSR;
}

/// Flags d'ouverture de fichier.
pub mod open_flags {
    pub const O_RDONLY: u32 = 0x0000;
    pub const O_WRONLY: u32 = 0x0001;
    pub const O_RDWR: u32 = 0x0002;
    pub const O_CREAT: u32 = 0x0040;
    pub const O_EXCL: u32 = 0x0080;
    pub const O_TRUNC: u32 = 0x0200;
    pub const O_APPEND: u32 = 0x0400;
    pub const O_NONBLOCK: u32 = 0x0800;
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures POSIX exposées
// ─────────────────────────────────────────────────────────────────────────────

/// Métadonnées d'un inode (stat-like).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VfsInode {
    pub ino: ObjectIno,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub link_count: u32,
    pub kind: u8,
    pub _pad: [u8; 3],
}

const _: () = assert!(core::mem::size_of::<VfsInode>() == 32);

impl VfsInode {
    pub fn is_dir(&self) -> bool {
        self.mode & file_mode::S_IFMT == file_mode::S_IFDIR
    }
    pub fn is_regular(&self) -> bool {
        self.mode & file_mode::S_IFMT == file_mode::S_IFREG
    }
    pub fn is_symlink(&self) -> bool {
        self.mode & file_mode::S_IFMT == file_mode::S_IFLNK
    }
    pub fn is_readable_by_owner(&self) -> bool {
        self.mode & file_mode::S_IRUSR != 0
    }
    pub fn is_writable_by_owner(&self) -> bool {
        self.mode & file_mode::S_IWUSR != 0
    }
}

/// Entrée de répertoire.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct VfsDirent {
    pub ino: ObjectIno,
    pub kind: u8,
    pub name_len: u8,
    pub _pad: [u8; 6],
    pub name: [u8; VFS_NAME_MAX],
}

const _: () = assert!(
    core::mem::size_of::<VfsDirent>() == 271,
    "VfsDirent ABI size changed — verifier les appels readdir() userspace"
);

impl VfsDirent {
    pub fn get_name(&self) -> &[u8] {
        let len = self.name_len as usize;
        &self.name[..len.min(VFS_NAME_MAX)]
    }
}

impl core::fmt::Debug for VfsDirent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let ino = self.ino;
        let kind = self.kind;
        let name_len = self.name_len;
        f.debug_struct("VfsDirent")
            .field("ino", &ino)
            .field("kind", &kind)
            .field("name_len", &name_len)
            .finish()
    }
}

/// Descripteur de fichier ouvert.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VfsFd {
    pub fd: u64,
    pub ino: ObjectIno,
    pub flags: u32,
    pub offset: u64,
    pub pid: u32,
    pub active: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Table des descripteurs ouverts
// ─────────────────────────────────────────────────────────────────────────────

struct OpenFdTable {
    fds: UnsafeCell<[Option<VfsFd>; VFS_OPEN_MAX]>,
    fd_count: AtomicUsize,
    spinlock: AtomicU64,
    next_fd: AtomicU64,
}

unsafe impl Sync for OpenFdTable {}
unsafe impl Send for OpenFdTable {}

impl OpenFdTable {
    const fn new() -> Self {
        Self {
            fds: UnsafeCell::new([None; VFS_OPEN_MAX]),
            fd_count: AtomicUsize::new(0),
            spinlock: AtomicU64::new(0),
            next_fd: AtomicU64::new(3), // 0=stdin,1=stdout,2=stderr
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

    fn open_fd(&self, ino: ObjectIno, flags: u32, pid: u32) -> ExofsResult<u64> {
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

    fn close_fd(&self, fd: u64) -> ExofsResult<()> {
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

    fn get_fd(&self, fd: u64) -> Option<VfsFd> {
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

    fn update_offset(&self, fd: u64, new_offset: u64) {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &mut *self.fds.get() };
        let mut i = 0usize;
        while i < fds.len() {
            if let Some(mut entry) = fds[i] {
                if entry.fd == fd && entry.active {
                    entry.offset = new_offset;
                    fds[i] = Some(entry);
                    break;
                }
            }
            i = i.wrapping_add(1);
        }
        self.lock_release();
    }

    fn close_all_pid(&self, pid: u32) {
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

    fn active_count(&self) -> usize {
        self.fd_count.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn reset_all(&self) {
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

static FD_TABLE: OpenFdTable = OpenFdTable::new();

#[inline]
fn current_vfs_owner_pid() -> u32 {
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
fn vfs_owner_matches(entry: VfsFd, caller: u32) -> bool {
    caller == 0 || entry.pid == 0 || entry.pid == caller
}

// ─────────────────────────────────────────────────────────────────────────────
// Registre VFS
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct DirRecord {
    ino: ObjectIno,
    kind: u8,
    name: Vec<u8>,
}

struct DirectoryRegistry {
    entries: UnsafeCell<BTreeMap<ObjectIno, Vec<DirRecord>>>,
    spinlock: AtomicU64,
}

unsafe impl Sync for DirectoryRegistry {}
unsafe impl Send for DirectoryRegistry {}

impl DirectoryRegistry {
    const fn new() -> Self {
        Self {
            entries: UnsafeCell::new(BTreeMap::new()),
            spinlock: AtomicU64::new(0),
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

    fn map(&self) -> &mut BTreeMap<ObjectIno, Vec<DirRecord>> {
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        unsafe { &mut *self.entries.get() }
    }

    fn ensure_dir(&self, ino: ObjectIno) -> ExofsResult<()> {
        self.lock_acquire();
        let map = self.map();
        if let Entry::Vacant(entry) = map.entry(ino) {
            entry.insert(Vec::new());
        }
        self.lock_release();
        Ok(())
    }

    fn lookup(&self, parent_ino: ObjectIno, name: &[u8]) -> Option<DirRecord> {
        self.lock_acquire();
        let result = self
            .map()
            .get(&parent_ino)
            .and_then(|records| records.iter().find(|record| record.name == name))
            .cloned();
        self.lock_release();
        result
    }

    fn insert(
        &self,
        parent_ino: ObjectIno,
        ino: ObjectIno,
        name: &[u8],
        kind: u8,
    ) -> ExofsResult<()> {
        let mut name_vec = Vec::new();
        name_vec
            .try_reserve(name.len())
            .map_err(|_| ExofsError::NoMemory)?;
        name_vec.extend_from_slice(name);

        self.lock_acquire();
        let map = self.map();
        if let Entry::Vacant(entry) = map.entry(parent_ino) {
            entry.insert(Vec::new());
        }
        let records = match map.get_mut(&parent_ino) {
            Some(records) => records,
            None => {
                self.lock_release();
                return Err(ExofsError::InternalError);
            }
        };
        if records.iter().any(|record| record.name == name) {
            self.lock_release();
            return Err(ExofsError::ObjectAlreadyExists);
        }
        if records.try_reserve(1).is_err() {
            self.lock_release();
            return Err(ExofsError::NoMemory);
        }
        records.push(DirRecord {
            ino,
            kind,
            name: name_vec,
        });
        self.lock_release();

        if kind == 4 {
            self.ensure_dir(ino)?;
        }
        Ok(())
    }

    fn list(&self, parent_ino: ObjectIno) -> ExofsResult<Vec<DirRecord>> {
        self.lock_acquire();
        let records = match self.map().get(&parent_ino).cloned() {
            Some(records) => records,
            None => {
                self.lock_release();
                return Err(ExofsError::ObjectNotFound);
            }
        };
        self.lock_release();
        Ok(records)
    }

    fn has_children(&self, parent_ino: ObjectIno) -> bool {
        self.lock_acquire();
        let result = self
            .map()
            .get(&parent_ino)
            .map(|records| !records.is_empty())
            .unwrap_or(false);
        self.lock_release();
        result
    }

    fn remove(&self, parent_ino: ObjectIno, name: &[u8]) -> Option<DirRecord> {
        self.lock_acquire();
        let map = self.map();
        let result = map.get_mut(&parent_ino).and_then(|records| {
            records
                .iter()
                .position(|record| record.name == name)
                .map(|idx| records.swap_remove(idx))
        });
        self.lock_release();
        result
    }

    fn rename(
        &self,
        old_parent: ObjectIno,
        old_name: &[u8],
        new_parent: ObjectIno,
        new_name: &[u8],
    ) -> ExofsResult<DirRecord> {
        let mut new_name_vec = Vec::new();
        new_name_vec
            .try_reserve(new_name.len())
            .map_err(|_| ExofsError::NoMemory)?;
        new_name_vec.extend_from_slice(new_name);

        self.lock_acquire();
        let map = self.map();
        if map
            .get(&new_parent)
            .map(|records| records.iter().any(|record| record.name == new_name))
            .unwrap_or(false)
        {
            self.lock_release();
            return Err(ExofsError::ObjectAlreadyExists);
        }
        let record = {
            let old_records = match map.get_mut(&old_parent) {
                Some(records) => records,
                None => {
                    self.lock_release();
                    return Err(ExofsError::ObjectNotFound);
                }
            };
            let idx = match old_records
                .iter()
                .position(|record| record.name == old_name)
            {
                Some(idx) => idx,
                None => {
                    self.lock_release();
                    return Err(ExofsError::ObjectNotFound);
                }
            };
            old_records.swap_remove(idx)
        };
        let mut updated = record;
        updated.name = new_name_vec;
        if let Entry::Vacant(entry) = map.entry(new_parent) {
            entry.insert(Vec::new());
        }
        let new_records = match map.get_mut(&new_parent) {
            Some(records) => records,
            None => {
                self.lock_release();
                return Err(ExofsError::InternalError);
            }
        };
        if new_records.try_reserve(1).is_err() {
            self.lock_release();
            return Err(ExofsError::NoMemory);
        }
        new_records.push(updated.clone());
        self.lock_release();
        Ok(updated)
    }

    fn remove_dir(&self, ino: ObjectIno) {
        self.lock_acquire();
        self.map().remove(&ino);
        self.lock_release();
    }

    #[cfg(test)]
    fn reset_all(&self) {
        self.lock_acquire();
        self.map().clear();
        self.map().insert(VFS_ROOT_INO, Vec::new());
        self.lock_release();
    }
}

static DIRECTORY_REGISTRY: DirectoryRegistry = DirectoryRegistry::new();
static VFS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Enregistre ExoFS comme opérateur VFS actif.
pub fn register_exofs_vfs_ops() -> ExofsResult<()> {
    if VFS_REGISTERED
        .compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)
        .is_err()
    {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    INODE_EMULATION.ensure_root()?;
    DIRECTORY_REGISTRY.ensure_dir(VFS_ROOT_INO)?;
    Ok(())
}

/// Retourne vrai si le VFS ExoFS est enregistré.
pub fn vfs_is_registered() -> bool {
    VFS_REGISTERED.load(Ordering::Acquire)
}

/// Retourne le numéro d'inode de la racine du FS.
pub fn root_inode() -> ObjectIno {
    VFS_ROOT_INO
}

// ─────────────────────────────────────────────────────────────────────────────
// Opérations VFS — surface POSIX
// ─────────────────────────────────────────────────────────────────────────────

/// Résout `name` dans le répertoire `parent_ino`. Retourne l'ino fils.
pub fn vfs_lookup(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<ObjectIno> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    // Vérifie que le parent est un répertoire.
    let parent = INODE_EMULATION
        .get_entry(parent_ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if parent.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    DIRECTORY_REGISTRY
        .lookup(parent_ino, name)
        .map(|record| record.ino)
        .ok_or(ExofsError::ObjectNotFound)
}

/// Crée le mapping d'un fichier sous `parent_ino`. Retourne l'ino créé.
pub fn vfs_create(
    parent_ino: ObjectIno,
    name: &[u8],
    mode: u32,
    uid: u64,
) -> ExofsResult<ObjectIno> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    validate_name(name)?;
    let parent = INODE_EMULATION
        .get_entry(parent_ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if parent.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    let oid = hash_name(parent_ino, name);
    // Vérifie qu'il n'existe pas déjà.
    if INODE_EMULATION.contains_oid(oid) {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    let flags = if mode & file_mode::S_IFMT == file_mode::S_IFDIR {
        inode_flags::DIRECTORY
    } else {
        inode_flags::REGULAR
    };
    let ino = INODE_EMULATION.get_or_alloc_flags(oid, flags, 0, uid)?;
    if let Err(err) = DIRECTORY_REGISTRY.insert(parent_ino, ino, name, inode_kind(flags)) {
        INODE_EMULATION.release_ino(ino);
        return Err(err);
    }
    Ok(ino)
}

/// Ouvre un inode et retourne un fd.
pub fn vfs_open(ino: ObjectIno, flags: u32, pid: u32) -> ExofsResult<u64> {
    let entry = INODE_EMULATION
        .get_entry(ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if flags & open_flags::O_WRONLY != 0 || flags & open_flags::O_RDWR != 0 {
        if entry.flags & inode_flags::READ_ONLY != 0 {
            return Err(ExofsError::PermissionDenied);
        }
    }
    if flags & open_flags::O_TRUNC != 0 {
        if entry.flags & inode_flags::DIRECTORY != 0 {
            return Err(ExofsError::WrongObjectKind);
        }
        resize_inode_data(&entry, 0)?;
        INODE_EMULATION.update_size(ino, 0)?;
    }
    FD_TABLE.open_fd(ino, flags, pid)
}

/// Ferme un fd.
pub fn vfs_close(fd: u64) -> ExofsResult<()> {
    FD_TABLE.close_fd(fd)
}

pub fn vfs_read(fd: u64, buf: &mut [u8], count: usize) -> ExofsResult<usize> {
    if count == 0 {
        return Ok(0);
    }
    let desc = FD_TABLE.get_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
    if desc.flags & open_flags::O_WRONLY != 0 {
        return Err(ExofsError::PermissionDenied);
    }
    let entry = INODE_EMULATION
        .get_entry(desc.ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY != 0 {
        return Err(ExofsError::WrongObjectKind);
    }
    let blob_id = ensure_inode_blob_cached(&entry)?;
    let start = desc.offset as usize;
    let current_len = BLOB_CACHE.len(&blob_id).unwrap_or(0);
    if start >= current_len {
        return Ok(0);
    }
    let readable = count.min(buf.len()).min(current_len.saturating_sub(start));
    if readable == 0 {
        return Ok(0);
    }
    let data = BLOB_CACHE.read_at(&blob_id, start, readable)?;
    buf[..readable].copy_from_slice(&data);
    let new_offset = desc.offset.saturating_add(readable as u64);
    FD_TABLE.update_offset(fd, new_offset);
    Ok(readable)
}

/// Écrit des données vers un fd.
pub fn vfs_write(fd: u64, buf: &[u8], count: usize) -> ExofsResult<usize> {
    if count == 0 {
        return Ok(0);
    }
    let desc = FD_TABLE.get_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
    if desc.flags & open_flags::O_WRONLY == 0 && desc.flags & open_flags::O_RDWR == 0 {
        return Err(ExofsError::PermissionDenied);
    }
    let entry = INODE_EMULATION
        .get_entry(desc.ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY != 0 {
        return Err(ExofsError::WrongObjectKind);
    }
    let written = count.min(buf.len());
    let blob_id = ensure_inode_blob_cached(&entry)?;
    let current_len = BLOB_CACHE.len(&blob_id).unwrap_or(0);
    let start_offset = if desc.flags & open_flags::O_APPEND != 0 {
        current_len
    } else {
        desc.offset as usize
    };
    let end_offset = start_offset
        .checked_add(written)
        .ok_or(ExofsError::OffsetOverflow)?;
    BLOB_CACHE.write_at(blob_id, start_offset, &buf[..written])?;
    let new_offset = end_offset as u64;
    FD_TABLE.update_offset(fd, new_offset);
    let new_size = (current_len.max(end_offset)) as u64;
    if new_size != entry.size {
        INODE_EMULATION.update_size(desc.ino, new_size)?;
    }
    Ok(written)
}

/// Retourne les métadonnées d'un inode.
pub fn vfs_getattr(ino: ObjectIno) -> ExofsResult<VfsInode> {
    let e = INODE_EMULATION
        .get_entry(ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    Ok(entry_to_vfs_inode(&e))
}

/// Crée un répertoire.
pub fn vfs_mkdir(
    parent_ino: ObjectIno,
    name: &[u8],
    mode: u32,
    uid: u64,
) -> ExofsResult<ObjectIno> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    validate_name(name)?;
    let dir_mode = (mode & !file_mode::S_IFMT) | file_mode::S_IFDIR;
    vfs_create(parent_ino, name, dir_mode, uid)
}

/// Supprime un fichier du répertoire parent.
pub fn vfs_unlink(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<()> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    let record = DIRECTORY_REGISTRY
        .remove(parent_ino, name)
        .ok_or(ExofsError::ObjectNotFound)?;
    let entry = INODE_EMULATION
        .get_entry(record.ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    // Ne peut pas unlink un répertoire.
    if entry.flags & inode_flags::DIRECTORY != 0 {
        let _ = DIRECTORY_REGISTRY.insert(parent_ino, record.ino, &record.name, record.kind);
        return Err(ExofsError::NotADirectory);
    }
    BLOB_CACHE.invalidate(&blob_id_for_object(entry.object_id));
    INODE_EMULATION.release_ino(record.ino);
    Ok(())
}

/// Supprime un répertoire (doit être vide).
pub fn vfs_rmdir(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<()> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    let record = DIRECTORY_REGISTRY
        .lookup(parent_ino, name)
        .ok_or(ExofsError::ObjectNotFound)?;
    let entry = INODE_EMULATION
        .get_entry(record.ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    if DIRECTORY_REGISTRY.has_children(record.ino) {
        return Err(ExofsError::DirectoryNotEmpty);
    }
    DIRECTORY_REGISTRY
        .remove(parent_ino, name)
        .ok_or(ExofsError::ObjectNotFound)?;
    DIRECTORY_REGISTRY.remove_dir(record.ino);
    INODE_EMULATION.release_ino(record.ino);
    Ok(())
}

/// Renomme `old_name` de `old_parent` vers `new_parent/new_name`.
pub fn vfs_rename(
    old_parent: ObjectIno,
    old_name: &[u8],
    new_parent: ObjectIno,
    new_name: &[u8],
) -> ExofsResult<()> {
    if old_name.is_empty() || new_name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if new_name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    validate_name(old_name)?;
    validate_name(new_name)?;
    // Vérifie que les deux parents sont des répertoires.
    let op = INODE_EMULATION
        .get_entry(old_parent)
        .ok_or(ExofsError::ObjectNotFound)?;
    let np = INODE_EMULATION
        .get_entry(new_parent)
        .ok_or(ExofsError::ObjectNotFound)?;
    if op.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    if np.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    let src = DIRECTORY_REGISTRY
        .lookup(old_parent, old_name)
        .ok_or(ExofsError::ObjectNotFound)?;
    let _ = DIRECTORY_REGISTRY.rename(old_parent, old_name, new_parent, new_name)?;
    let _ = (op, np, src);
    Ok(())
}

/// Retourne un vecteur de VfsDirent pour un ino répertoire.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn vfs_readdir(parent_ino: ObjectIno, _offset: u64) -> ExofsResult<Vec<VfsDirent>> {
    let entry = INODE_EMULATION
        .get_entry(parent_ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY == 0 {
        return Err(ExofsError::NotADirectory);
    }
    let records = DIRECTORY_REGISTRY.list(parent_ino)?;
    let mut out: Vec<VfsDirent> = Vec::new();
    out.try_reserve(records.len().saturating_add(2))
        .map_err(|_| ExofsError::NoMemory)?;
    // Toujours inclure "." et ".."
    let dot = make_dirent(parent_ino, b".", 4);
    let dotdot = make_dirent(VFS_ROOT_INO, b"..", 4);
    out.push(dot);
    out.push(dotdot);
    let mut i = 0usize;
    while i < records.len() {
        let record = &records[i];
        out.push(make_dirent(record.ino, &record.name, record.kind));
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Tronque/étend un fichier à `new_size`.
pub fn vfs_truncate(ino: ObjectIno, new_size: u64) -> ExofsResult<()> {
    let e = INODE_EMULATION
        .get_entry(ino)
        .ok_or(ExofsError::ObjectNotFound)?;
    if e.flags & inode_flags::DIRECTORY != 0 {
        return Err(ExofsError::WrongObjectKind);
    }
    if new_size > usize::MAX as u64 {
        return Err(ExofsError::OffsetOverflow);
    }
    let new_len = new_size as usize;
    resize_inode_data(&e, new_len)?;
    INODE_EMULATION.update_size(ino, new_size)
}

/// Crée un lien symbolique.
pub fn vfs_symlink(parent_ino: ObjectIno, name: &[u8], uid: u64) -> ExofsResult<ObjectIno> {
    if name.is_empty() {
        return Err(ExofsError::InvalidArgument);
    }
    if name.len() > VFS_NAME_MAX {
        return Err(ExofsError::PathTooLong);
    }
    validate_name(name)?;
    let oid = hash_name(parent_ino, name);
    if INODE_EMULATION.contains_oid(oid) {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    let ino = INODE_EMULATION.get_or_alloc_flags(oid, inode_flags::SYMLINK, 0, uid)?;
    if let Err(err) =
        DIRECTORY_REGISTRY.insert(parent_ino, ino, name, inode_kind(inode_flags::SYMLINK))
    {
        INODE_EMULATION.release_ino(ino);
        return Err(err);
    }
    Ok(ino)
}

/// Ferme tous les fd d'un pid (exit/kill).
pub fn vfs_close_all_pid(pid: u32) {
    FD_TABLE.close_all_pid(pid);
}

/// Nombre de descripteurs ouverts.
pub fn vfs_open_count() -> usize {
    FD_TABLE.active_count()
}

#[cfg(test)]
pub fn reset_vfs_state_for_test() {
    FD_TABLE.reset_all();
    DIRECTORY_REGISTRY.reset_all();
    INODE_EMULATION.clear();
    BLOB_CACHE.flush_all_force();
    object_store::OBJECT_STORE.reset_all();
    VFS_REGISTERED.store(false, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers internes
// ─────────────────────────────────────────────────────────────────────────────

fn blob_id_for_object(object_id: u64) -> BlobId {
    BlobId::from_bytes_blake3(&object_id.to_le_bytes())
}

fn ensure_inode_blob_cached(entry: &InodeEntry) -> ExofsResult<BlobId> {
    let blob_id = blob_id_for_object(entry.object_id);
    if BLOB_CACHE.contains(&blob_id) {
        return Ok(blob_id);
    }
    if let Some(data) = object_store::load_blob_data_if_available(&blob_id)? {
        BLOB_CACHE.insert(blob_id, data)?;
    } else {
        BLOB_CACHE.insert(blob_id, Vec::new())?;
    }
    Ok(blob_id)
}

fn resize_inode_data(entry: &InodeEntry, new_len: usize) -> ExofsResult<()> {
    let blob_id = ensure_inode_blob_cached(entry)?;
    BLOB_CACHE.resize(blob_id, new_len)?;
    Ok(())
}

/// Convertit un InodeEntry en VfsInode.
fn entry_to_vfs_inode(e: &InodeEntry) -> VfsInode {
    let mode = if e.flags & inode_flags::DIRECTORY != 0 {
        file_mode::DEFAULT_DIR
    } else if e.flags & inode_flags::SYMLINK != 0 {
        file_mode::S_IFLNK | 0o777
    } else {
        file_mode::DEFAULT_FILE
    };
    VfsInode {
        ino: e.ino,
        size: e.size,
        mode,
        uid: e.uid as u32,
        link_count: e.link_count,
        kind: inode_kind(e.flags),
        _pad: [0; 3],
    }
}

fn inode_kind(flags: u32) -> u8 {
    if flags & inode_flags::DIRECTORY != 0 {
        4
    } else if flags & inode_flags::SYMLINK != 0 {
        10
    } else {
        8
    }
}

/// Construit un VfsDirent depuis un ino et un nom.
fn make_dirent(ino: ObjectIno, name: &[u8], kind: u8) -> VfsDirent {
    let mut d = VfsDirent {
        ino,
        kind,
        name_len: name.len().min(VFS_NAME_MAX) as u8,
        _pad: [0; 6],
        name: [0; VFS_NAME_MAX],
    };
    let mut i = 0usize;
    while i < d.name_len as usize {
        d.name[i] = name[i];
        i = i.wrapping_add(1);
    }
    d
}

/// Valide un composant de nom (pas de '/', '\0', pas ".." admis ici).
fn validate_name(name: &[u8]) -> ExofsResult<()> {
    let mut i = 0usize;
    while i < name.len() {
        if name[i] == b'/' || name[i] == 0 {
            return Err(ExofsError::InvalidPathComponent);
        }
        i = i.wrapping_add(1);
    }
    Ok(())
}

/// Hash de nom déterministe : FNV-1a 64-bit sur (parent_ino, name).
/// ARITH-02 : wrapping_mul/xor.
fn hash_name(parent_ino: ObjectIno, name: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET;
    let pid_bytes = parent_ino.to_le_bytes();
    let mut i = 0usize;
    while i < 8 {
        h = h.wrapping_mul(FNV_PRIME) ^ (pid_bytes[i] as u64);
        i = i.wrapping_add(1);
    }
    let mut j = 0usize;
    while j < name.len() {
        h = h.wrapping_mul(FNV_PRIME) ^ (name[j] as u64);
        j = j.wrapping_add(1);
    }
    // Évite oid == 0.
    if h == 0 {
        1
    } else {
        h
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

    #[test]
    fn test_vfs_inode_size() {
        assert_eq!(core::mem::size_of::<VfsInode>(), 32);
    }

    #[test]
    fn test_vfs_dirent_size() {
        assert_eq!(core::mem::size_of::<VfsDirent>(), 271);
    }

    #[test]
    fn test_hash_name_stable() {
        let h1 = hash_name(1, b"foo");
        let h2 = hash_name(1, b"foo");
        let h3 = hash_name(1, b"bar");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_hash_name_nonzero() {
        // Vérifie que le résultat n'est jamais 0.
        let h = hash_name(0, b"\x00");
        assert_ne!(h, 0);
    }

    #[test]
    fn test_validate_name_ok() {
        assert!(validate_name(b"hello").is_ok());
    }

    #[test]
    fn test_validate_name_slash() {
        assert!(matches!(
            validate_name(b"a/b"),
            Err(ExofsError::InvalidPathComponent)
        ));
    }

    #[test]
    fn test_validate_name_null() {
        assert!(matches!(
            validate_name(b"a\x00b"),
            Err(ExofsError::InvalidPathComponent)
        ));
    }

    #[test]
    fn test_make_dirent() {
        let d = make_dirent(42, b"hello", 8);
        let ino = d.ino;
        let name_len = d.name_len;
        assert_eq!(ino, 42);
        assert_eq!(name_len, 5);
        assert_eq!(&d.name[..5], b"hello");
    }

    #[test]
    fn test_entry_to_vfs_inode_dir() {
        let e = crate::fs::exofs::posix_bridge::inode_emulation::InodeEntry {
            ino: 1,
            object_id: 1,
            flags: inode_flags::DIRECTORY,
            link_count: 2,
            size: 0,
            uid: 0,
            epoch_id: 0,
            access_ts: 0,
        };
        let v = entry_to_vfs_inode(&e);
        assert!(v.is_dir());
    }

    #[test]
    fn test_entry_to_vfs_inode_file() {
        let e = crate::fs::exofs::posix_bridge::inode_emulation::InodeEntry {
            ino: 10,
            object_id: 10,
            flags: inode_flags::REGULAR,
            link_count: 1,
            size: 1024,
            uid: 1000,
            epoch_id: 0,
            access_ts: 0,
        };
        let v = entry_to_vfs_inode(&e);
        assert!(v.is_regular());
        assert!(!v.is_dir());
    }

    #[test]
    fn test_inode_kind_values() {
        assert_eq!(inode_kind(inode_flags::DIRECTORY), 4);
        assert_eq!(inode_kind(inode_flags::REGULAR), 8);
        assert_eq!(inode_kind(inode_flags::SYMLINK), 10);
    }

    #[test]
    fn test_fd_table_open_close() {
        let fdt = OpenFdTable::new();
        let fd = fdt.open_fd(5, open_flags::O_RDONLY, 1).test_unwrap();
        assert!(fdt.get_fd(fd).is_some());
        fdt.close_fd(fd).test_unwrap();
        assert!(fdt.get_fd(fd).is_none());
    }

    #[test]
    fn test_fd_table_update_offset() {
        let fdt = OpenFdTable::new();
        let fd = fdt.open_fd(7, open_flags::O_RDONLY, 1).test_unwrap();
        fdt.update_offset(fd, 512);
        let d = fdt.get_fd(fd).test_unwrap();
        assert_eq!(d.offset, 512);
        fdt.close_fd(fd).test_unwrap();
    }
}
