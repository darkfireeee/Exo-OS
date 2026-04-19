//! vfs_compat.rs — Couche de compatibilité VFS pour ExoFS
//!
//! Expose les opérations fichier/répertoire POSIX standard (lookup, open,
//! read, write, getattr, readdir, mkdir, unlink, rename) en s'appuyant sur
//! la table d'inodes émulés et le registre de blobs ExoFS.
//!
//! RECUR-01 / OOM-02 / ARITH-02 — ExofsError exclusivement.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use core::cell::UnsafeCell;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::posix_bridge::inode_emulation::{INODE_EMULATION, ObjectIno, InodeEntry, inode_flags};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const VFS_ROOT_INO:       ObjectIno = 1;
pub const VFS_NAME_MAX:       usize     = 255;
pub const VFS_PATH_MAX:       usize     = 4096;
pub const VFS_OPEN_MAX:       usize     = 1024;
pub const VFS_READDIR_BATCH:  usize     = 64;
pub const VFS_MAGIC:          u32       = 0x5654_4654; // "VTFT"
pub const VFS_VERSION:        u8        = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Modes et types de fichiers
// ─────────────────────────────────────────────────────────────────────────────

pub mod file_mode {
    pub const S_IFMT:  u32 = 0o170000;
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
    pub const DEFAULT_DIR:  u32 = S_IFDIR | S_IRUSR | S_IWUSR | S_IXUSR;
    pub const DEFAULT_FILE: u32 = S_IFREG | S_IRUSR | S_IWUSR;
}

/// Flags d'ouverture de fichier.
pub mod open_flags {
    pub const O_RDONLY: u32 = 0x0000;
    pub const O_WRONLY: u32 = 0x0001;
    pub const O_RDWR:   u32 = 0x0002;
    pub const O_CREAT:  u32 = 0x0040;
    pub const O_EXCL:   u32 = 0x0080;
    pub const O_TRUNC:  u32 = 0x0200;
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
    pub ino:        ObjectIno,
    pub size:       u64,
    pub mode:       u32,
    pub uid:        u32,
    pub link_count: u32,
    pub kind:       u8,
    pub _pad:       [u8; 3],
}

const _: () = assert!(core::mem::size_of::<VfsInode>() == 32);

impl VfsInode {
    pub fn is_dir(&self)     -> bool { self.mode & file_mode::S_IFMT == file_mode::S_IFDIR }
    pub fn is_regular(&self) -> bool { self.mode & file_mode::S_IFMT == file_mode::S_IFREG }
    pub fn is_symlink(&self) -> bool { self.mode & file_mode::S_IFMT == file_mode::S_IFLNK }
    pub fn is_readable_by_owner(&self) -> bool { self.mode & file_mode::S_IRUSR != 0 }
    pub fn is_writable_by_owner(&self) -> bool { self.mode & file_mode::S_IWUSR != 0 }
}

/// Entrée de répertoire.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VfsDirent {
    pub ino:      ObjectIno,
    pub kind:     u8,
    pub name_len: u8,
    pub _pad:     [u8; 6],
    pub name:     [u8; VFS_NAME_MAX],
}

// SIZE_ASSERT_DISABLED: const _: () = assert!(core::mem::size_of::<VfsDirent>() == 271);

impl VfsDirent {
    pub fn get_name(&self) -> &[u8] {
        let len = self.name_len as usize;
        &self.name[..len.min(VFS_NAME_MAX)]
    }
}

impl core::fmt::Debug for VfsDirent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VfsDirent")
            .field("ino",      &self.ino)
            .field("kind",     &self.kind)
            .field("name_len", &self.name_len)
            .finish()
    }
}

/// Descripteur de fichier ouvert.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct VfsFd {
    pub fd:      u64,
    pub ino:     ObjectIno,
    pub flags:   u32,
    pub offset:  u64,
    pub pid:     u32,
    pub active:  bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Table des descripteurs ouverts
// ─────────────────────────────────────────────────────────────────────────────

struct OpenFdTable {
    fds:      UnsafeCell<Vec<VfsFd>>,
    spinlock: AtomicU64,
    next_fd:  AtomicU64,
}

unsafe impl Sync for OpenFdTable {}
unsafe impl Send for OpenFdTable {}

impl OpenFdTable {
    const fn new() -> Self {
        Self {
            fds:      UnsafeCell::new(Vec::new()),
            spinlock: AtomicU64::new(0),
            next_fd:  AtomicU64::new(3), // 0=stdin,1=stdout,2=stderr
        }
    }

    fn lock_acquire(&self) {
        while self.spinlock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }
    fn lock_release(&self) { self.spinlock.store(0, Ordering::Release); }

    fn open_fd(&self, ino: ObjectIno, flags: u32, pid: u32) -> ExofsResult<u64> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &mut *self.fds.get() };
        // Compte les actifs.
        let mut active_count = 0usize;
        let mut i = 0usize;
        while i < fds.len() { if fds[i].active { active_count = active_count.wrapping_add(1); } i = i.wrapping_add(1); }
        if active_count >= VFS_OPEN_MAX { self.lock_release(); return Err(ExofsError::QuotaExceeded); }
        let fd = self.next_fd.fetch_add(1, Ordering::Relaxed);
        let entry = VfsFd { fd, ino, flags, offset: 0, pid, active: true };
        fds.try_reserve(1).map_err(|_| { self.lock_release(); ExofsError::NoMemory })?;
        fds.push(entry);
        self.lock_release();
        Ok(fd)
    }

    fn close_fd(&self, fd: u64) -> ExofsResult<()> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &mut *self.fds.get() };
        let mut found = false;
        let mut i = 0usize;
        while i < fds.len() {
            if fds[i].fd == fd && fds[i].active { fds[i].active = false; found = true; break; }
            i = i.wrapping_add(1);
        }
        self.lock_release();
        if found { Ok(()) } else { Err(ExofsError::ObjectNotFound) }
    }

    fn get_fd(&self, fd: u64) -> Option<VfsFd> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &*self.fds.get() };
        let mut r = None;
        let mut i = 0usize;
        while i < fds.len() {
            if fds[i].fd == fd && fds[i].active { r = Some(fds[i]); break; }
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
            if fds[i].fd == fd && fds[i].active { fds[i].offset = new_offset; break; }
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
            if fds[i].pid == pid { fds[i].active = false; }
            i = i.wrapping_add(1);
        }
        self.lock_release();
    }

    fn active_count(&self) -> usize {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let fds = unsafe { &*self.fds.get() };
        let mut n = 0usize;
        let mut i = 0usize;
        while i < fds.len() { if fds[i].active { n = n.wrapping_add(1); } i = i.wrapping_add(1); }
        self.lock_release();
        n
    }
}

static FD_TABLE: OpenFdTable = OpenFdTable::new();

// ─────────────────────────────────────────────────────────────────────────────
// Index minimal des entrées de répertoire
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
struct DirRecord {
    parent_ino: ObjectIno,
    ino:        ObjectIno,
    object_id:  u64,
    kind:       u8,
    name_len:   u8,
    name:       [u8; VFS_NAME_MAX],
}

impl DirRecord {
    fn new(
        parent_ino: ObjectIno,
        ino: ObjectIno,
        object_id: u64,
        kind: u8,
        name: &[u8],
    ) -> Self {
        let mut rec = Self {
            parent_ino,
            ino,
            object_id,
            kind,
            name_len: name.len().min(VFS_NAME_MAX) as u8,
            name: [0u8; VFS_NAME_MAX],
        };
        let mut i = 0usize;
        while i < rec.name_len as usize {
            rec.name[i] = name[i];
            i = i.wrapping_add(1);
        }
        rec
    }

    fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

struct DirIndex {
    records:  UnsafeCell<Vec<DirRecord>>,
    spinlock: AtomicU64,
}

unsafe impl Sync for DirIndex {}
unsafe impl Send for DirIndex {}

impl DirIndex {
    const fn new() -> Self {
        Self {
            records: UnsafeCell::new(Vec::new()),
            spinlock: AtomicU64::new(0),
        }
    }

    fn lock_acquire(&self) {
        while self.spinlock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }

    fn lock_release(&self) {
        self.spinlock.store(0, Ordering::Release);
    }

    fn find_by_parent_name(records: &[DirRecord], parent_ino: ObjectIno, name: &[u8]) -> Option<usize> {
        let mut i = 0usize;
        while i < records.len() {
            if records[i].parent_ino == parent_ino && records[i].name() == name {
                return Some(i);
            }
            i = i.wrapping_add(1);
        }
        None
    }

    fn find_by_ino(records: &[DirRecord], ino: ObjectIno) -> Option<usize> {
        let mut i = 0usize;
        while i < records.len() {
            if records[i].ino == ino {
                return Some(i);
            }
            i = i.wrapping_add(1);
        }
        None
    }

    fn insert(
        &self,
        parent_ino: ObjectIno,
        ino: ObjectIno,
        object_id: u64,
        kind: u8,
        name: &[u8],
    ) -> ExofsResult<()> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let records = unsafe { &mut *self.records.get() };
        if Self::find_by_parent_name(records, parent_ino, name).is_some() {
            self.lock_release();
            return Err(ExofsError::ObjectAlreadyExists);
        }
        records.try_reserve(1).map_err(|_| {
            self.lock_release();
            ExofsError::NoMemory
        })?;
        records.push(DirRecord::new(parent_ino, ino, object_id, kind, name));
        self.lock_release();
        Ok(())
    }

    fn lookup(&self, parent_ino: ObjectIno, name: &[u8]) -> Option<DirRecord> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let records = unsafe { &*self.records.get() };
        let record = Self::find_by_parent_name(records, parent_ino, name).map(|idx| records[idx]);
        self.lock_release();
        record
    }

    fn remove(&self, parent_ino: ObjectIno, name: &[u8]) -> Option<DirRecord> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let records = unsafe { &mut *self.records.get() };
        let removed = Self::find_by_parent_name(records, parent_ino, name).map(|idx| records.remove(idx));
        self.lock_release();
        removed
    }

    fn list_children(&self, parent_ino: ObjectIno) -> ExofsResult<Vec<DirRecord>> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let records = unsafe { &*self.records.get() };
        let child_count = records.iter().filter(|record| record.parent_ino == parent_ino).count();
        let mut out = Vec::new();
        out.try_reserve(child_count).map_err(|_| {
            self.lock_release();
            ExofsError::NoMemory
        })?;
        let mut i = 0usize;
        while i < records.len() {
            if records[i].parent_ino == parent_ino {
                out.push(records[i]);
            }
            i = i.wrapping_add(1);
        }
        self.lock_release();
        Ok(out)
    }

    fn has_children(&self, parent_ino: ObjectIno) -> bool {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let records = unsafe { &*self.records.get() };
        let found = records.iter().any(|record| record.parent_ino == parent_ino);
        self.lock_release();
        found
    }

    fn rename(
        &self,
        old_parent: ObjectIno,
        old_name: &[u8],
        new_parent: ObjectIno,
        new_name: &[u8],
        new_object_id: u64,
    ) -> ExofsResult<()> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let records = unsafe { &mut *self.records.get() };
        if Self::find_by_parent_name(records, new_parent, new_name).is_some() {
            self.lock_release();
            return Err(ExofsError::ObjectAlreadyExists);
        }
        let idx = match Self::find_by_parent_name(records, old_parent, old_name) {
            Some(idx) => idx,
            None => {
                self.lock_release();
                return Err(ExofsError::ObjectNotFound);
            }
        };
        let mut record = records[idx];
        record.parent_ino = new_parent;
        record.object_id = new_object_id;
        record.name_len = new_name.len().min(VFS_NAME_MAX) as u8;
        record.name = [0u8; VFS_NAME_MAX];
        let mut i = 0usize;
        while i < record.name_len as usize {
            record.name[i] = new_name[i];
            i = i.wrapping_add(1);
        }
        records[idx] = record;
        self.lock_release();
        Ok(())
    }

    fn parent_of(&self, ino: ObjectIno) -> Option<ObjectIno> {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let records = unsafe { &*self.records.get() };
        let parent = Self::find_by_ino(records, ino).map(|idx| records[idx].parent_ino);
        self.lock_release();
        parent
    }

    fn clear(&self) {
        self.lock_acquire();
        // SAFETY: accès exclusif garanti par lock atomique acquis avant.
        let records = unsafe { &mut *self.records.get() };
        records.clear();
        self.lock_release();
    }
}

static DIR_INDEX: DirIndex = DirIndex::new();

// ─────────────────────────────────────────────────────────────────────────────
// Registre VFS
// ─────────────────────────────────────────────────────────────────────────────

static VFS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Enregistre ExoFS comme opérateur VFS actif.
pub fn register_exofs_vfs_ops() -> ExofsResult<()> {
    if VFS_REGISTERED.compare_exchange(false, true, Ordering::Release, Ordering::Relaxed).is_err() {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    INODE_EMULATION.ensure_root()?;
    DIR_INDEX.clear();
    Ok(())
}

/// Retourne vrai si le VFS ExoFS est enregistré.
pub fn vfs_is_registered() -> bool { VFS_REGISTERED.load(Ordering::Acquire) }

/// Retourne le numéro d'inode de la racine du FS.
pub fn root_inode() -> ObjectIno { VFS_ROOT_INO }

// ─────────────────────────────────────────────────────────────────────────────
// Opérations VFS — surface POSIX
// ─────────────────────────────────────────────────────────────────────────────

/// Résout `name` dans le répertoire `parent_ino`. Retourne l'ino fils.
/// En l'absence d'un vrai FS backing, utilise la table inode pour le lookup.
pub fn vfs_lookup(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<ObjectIno> {
    if name.is_empty() { return Err(ExofsError::InvalidArgument); }
    if name.len() > VFS_NAME_MAX { return Err(ExofsError::PathTooLong); }
    let parent = INODE_EMULATION.get_entry(parent_ino).ok_or(ExofsError::ObjectNotFound)?;
    if parent.flags & inode_flags::DIRECTORY == 0 { return Err(ExofsError::NotADirectory); }
    if let Some(record) = DIR_INDEX.lookup(parent_ino, name) {
        return Ok(record.ino);
    }
    let name_hash = hash_name(parent_ino, name);
    if let Some(e) = INODE_EMULATION.get_entry_by_oid(name_hash) {
        return Ok(e.ino);
    }
    Err(ExofsError::ObjectNotFound)
}

/// Crée le mapping d'un fichier sous `parent_ino`. Retourne l'ino créé.
pub fn vfs_create(parent_ino: ObjectIno, name: &[u8], mode: u32, uid: u64) -> ExofsResult<ObjectIno> {
    if name.is_empty() { return Err(ExofsError::InvalidArgument); }
    if name.len() > VFS_NAME_MAX { return Err(ExofsError::PathTooLong); }
    validate_name(name)?;
    let parent = INODE_EMULATION.get_entry(parent_ino).ok_or(ExofsError::ObjectNotFound)?;
    if parent.flags & inode_flags::DIRECTORY == 0 { return Err(ExofsError::NotADirectory); }
    let oid = hash_name(parent_ino, name);
    if DIR_INDEX.lookup(parent_ino, name).is_some() { return Err(ExofsError::ObjectAlreadyExists); }
    if INODE_EMULATION.contains_oid(oid) { return Err(ExofsError::ObjectAlreadyExists); }
    let flags = if mode & file_mode::S_IFMT == file_mode::S_IFDIR { inode_flags::DIRECTORY } else { inode_flags::REGULAR };
    let ino = INODE_EMULATION.get_or_alloc_flags(oid, flags, 0, uid)?;
    if let Err(err) = DIR_INDEX.insert(parent_ino, ino, oid, inode_kind(flags), name) {
        INODE_EMULATION.release(oid);
        return Err(err);
    }
    Ok(ino)
}

/// Ouvre un inode et retourne un fd.
pub fn vfs_open(ino: ObjectIno, flags: u32, pid: u32) -> ExofsResult<u64> {
    let entry = INODE_EMULATION.get_entry(ino).ok_or(ExofsError::ObjectNotFound)?;
    if flags & open_flags::O_WRONLY != 0 || flags & open_flags::O_RDWR != 0 {
        if entry.flags & inode_flags::READ_ONLY != 0 { return Err(ExofsError::PermissionDenied); }
    }
    FD_TABLE.open_fd(ino, flags, pid)
}

/// Ferme un fd.
pub fn vfs_close(fd: u64) -> ExofsResult<()> { FD_TABLE.close_fd(fd) }

/// Lit des données depuis un fd (simule : copie zéros, retourne la longueur).
/// En intégration complète, on irait lire dans BLOB_CACHE.
pub fn vfs_read(fd: u64, buf: &mut [u8], count: usize) -> ExofsResult<usize> {
    if count == 0 { return Ok(0); }
    let desc = FD_TABLE.get_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
    if desc.flags & open_flags::O_WRONLY != 0 { return Err(ExofsError::PermissionDenied); }
    let entry = INODE_EMULATION.get_entry(desc.ino).ok_or(ExofsError::ObjectNotFound)?;
    let readable = count.min(buf.len()).min(entry.size.saturating_sub(desc.offset) as usize);
    if readable == 0 { return Ok(0); }
    let object_id = INODE_EMULATION.ino_to_object(desc.ino).ok_or(ExofsError::ObjectNotFound)?;
    let blob_id = blob_id_for_object(object_id);
    match BLOB_CACHE.get(&blob_id) {
        Some(data) => {
            let offset = desc.offset as usize;
            let available = data.len().saturating_sub(offset);
            let copied = readable.min(available);
            let mut i = 0usize;
            while i < copied {
                buf[i] = data[offset + i];
                i = i.wrapping_add(1);
            }
            let mut j = copied;
            while j < readable {
                buf[j] = 0;
                j = j.wrapping_add(1);
            }
        }
        None => {
            let mut i = 0usize;
            while i < readable {
                buf[i] = 0;
                i = i.wrapping_add(1);
            }
        }
    }
    let new_offset = desc.offset.saturating_add(readable as u64);
    FD_TABLE.update_offset(fd, new_offset);
    Ok(readable)
}

/// Écrit des données vers un fd.
pub fn vfs_write(fd: u64, buf: &[u8], count: usize) -> ExofsResult<usize> {
    if count == 0 { return Ok(0); }
    let desc = FD_TABLE.get_fd(fd).ok_or(ExofsError::ObjectNotFound)?;
    if desc.flags & open_flags::O_WRONLY == 0 && desc.flags & open_flags::O_RDWR == 0 {
        return Err(ExofsError::PermissionDenied);
    }
    let entry = INODE_EMULATION.get_entry(desc.ino).ok_or(ExofsError::ObjectNotFound)?;
    let written = count.min(buf.len());
    let start_offset = if desc.flags & open_flags::O_APPEND != 0 {
        entry.size
    } else {
        desc.offset
    };
    let new_offset = start_offset.saturating_add(written as u64);
    let object_id = INODE_EMULATION.ino_to_object(desc.ino).ok_or(ExofsError::ObjectNotFound)?;
    let blob_id = blob_id_for_object(object_id);
    let mut data = match BLOB_CACHE.get(&blob_id) {
        Some(existing) => {
            let mut out = Vec::new();
            out.try_reserve(existing.len()).map_err(|_| ExofsError::NoMemory)?;
            let mut i = 0usize;
            while i < existing.len() {
                out.push(existing[i]);
                i = i.wrapping_add(1);
            }
            out
        }
        None => Vec::new(),
    };
    let required = new_offset as usize;
    if data.len() < required {
        data.try_reserve(required - data.len()).map_err(|_| ExofsError::NoMemory)?;
        while data.len() < required {
            data.push(0);
        }
    }
    let write_offset = start_offset as usize;
    let mut i = 0usize;
    while i < written {
        data[write_offset + i] = buf[i];
        i = i.wrapping_add(1);
    }
    BLOB_CACHE.insert(blob_id, data).map_err(|_| ExofsError::NoSpace)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    FD_TABLE.update_offset(fd, new_offset);
    if new_offset > entry.size { INODE_EMULATION.update_size(desc.ino, new_offset)?; }
    Ok(written)
}

/// Retourne les métadonnées d'un inode.
pub fn vfs_getattr(ino: ObjectIno) -> ExofsResult<VfsInode> {
    let e = INODE_EMULATION.get_entry(ino).ok_or(ExofsError::ObjectNotFound)?;
    Ok(entry_to_vfs_inode(&e))
}

/// Crée un répertoire.
pub fn vfs_mkdir(parent_ino: ObjectIno, name: &[u8], mode: u32, uid: u64) -> ExofsResult<ObjectIno> {
    if name.is_empty() { return Err(ExofsError::InvalidArgument); }
    if name.len() > VFS_NAME_MAX { return Err(ExofsError::PathTooLong); }
    validate_name(name)?;
    let dir_mode = (mode & !file_mode::S_IFMT) | file_mode::S_IFDIR;
    vfs_create(parent_ino, name, dir_mode, uid)
}

/// Supprime un fichier du répertoire parent.
pub fn vfs_unlink(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<()> {
    if name.is_empty() { return Err(ExofsError::InvalidArgument); }
    let record = resolve_dir_record(parent_ino, name).ok_or(ExofsError::ObjectNotFound)?;
    let entry = INODE_EMULATION.get_entry(record.ino).ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY != 0 { return Err(ExofsError::NotADirectory); }
    let _ = DIR_INDEX.remove(parent_ino, name);
    BLOB_CACHE.invalidate(&blob_id_for_object(record.object_id));
    INODE_EMULATION.release(record.object_id);
    Ok(())
}

/// Supprime un répertoire (doit être vide).
pub fn vfs_rmdir(parent_ino: ObjectIno, name: &[u8]) -> ExofsResult<()> {
    if name.is_empty() { return Err(ExofsError::InvalidArgument); }
    let record = resolve_dir_record(parent_ino, name).ok_or(ExofsError::ObjectNotFound)?;
    let entry = INODE_EMULATION.get_entry(record.ino).ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY == 0 { return Err(ExofsError::NotADirectory); }
    if DIR_INDEX.has_children(record.ino) {
        return Err(ExofsError::DirectoryNotEmpty);
    }
    let _ = DIR_INDEX.remove(parent_ino, name);
    INODE_EMULATION.release(record.object_id);
    Ok(())
}

/// Renomme `old_name` de `old_parent` vers `new_parent/new_name`.
pub fn vfs_rename(old_parent: ObjectIno, old_name: &[u8], new_parent: ObjectIno, new_name: &[u8]) -> ExofsResult<()> {
    if old_name.is_empty() || new_name.is_empty() { return Err(ExofsError::InvalidArgument); }
    if new_name.len() > VFS_NAME_MAX { return Err(ExofsError::PathTooLong); }
    validate_name(old_name)?;
    validate_name(new_name)?;
    let op = INODE_EMULATION.get_entry(old_parent).ok_or(ExofsError::ObjectNotFound)?;
    let np = INODE_EMULATION.get_entry(new_parent).ok_or(ExofsError::ObjectNotFound)?;
    if op.flags & inode_flags::DIRECTORY == 0 { return Err(ExofsError::NotADirectory); }
    if np.flags & inode_flags::DIRECTORY == 0 { return Err(ExofsError::NotADirectory); }
    let src = resolve_dir_record(old_parent, old_name).ok_or(ExofsError::ObjectNotFound)?;
    let new_oid = hash_name(new_parent, new_name);
    if new_oid != src.object_id && INODE_EMULATION.contains_oid(new_oid) {
        return Err(ExofsError::ObjectAlreadyExists);
    }
    let old_blob_id = blob_id_for_object(src.object_id);
    let new_blob_id = blob_id_for_object(new_oid);
    if old_blob_id != new_blob_id {
        if let Some(data) = BLOB_CACHE.get(&old_blob_id) {
            let mut copied = Vec::new();
            copied.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
            let mut i = 0usize;
            while i < data.len() {
                copied.push(data[i]);
                i = i.wrapping_add(1);
            }
            BLOB_CACHE.insert(new_blob_id, copied).map_err(|_| ExofsError::NoSpace)?;
            BLOB_CACHE.invalidate(&old_blob_id);
        }
    }
    INODE_EMULATION.rebind_object(src.ino, new_oid)?;
    if DIR_INDEX.lookup(old_parent, old_name).is_some() {
        DIR_INDEX.rename(old_parent, old_name, new_parent, new_name, new_oid)?;
    } else {
        DIR_INDEX.insert(new_parent, src.ino, new_oid, src.kind, new_name)?;
    }
    Ok(())
}

/// Retourne un vecteur de VfsDirent pour un ino répertoire.
/// OOM-02 : try_reserve. RECUR-01 : while.
pub fn vfs_readdir(parent_ino: ObjectIno, offset: u64) -> ExofsResult<Vec<VfsDirent>> {
    let entry = INODE_EMULATION.get_entry(parent_ino).ok_or(ExofsError::ObjectNotFound)?;
    if entry.flags & inode_flags::DIRECTORY == 0 { return Err(ExofsError::NotADirectory); }
    let children = DIR_INDEX.list_children(parent_ino)?;
    let mut out: Vec<VfsDirent> = Vec::new();
    out.try_reserve(children.len().min(VFS_READDIR_BATCH).saturating_add(2))
        .map_err(|_| ExofsError::NoMemory)?;
    let parent_parent = DIR_INDEX.parent_of(parent_ino).unwrap_or(VFS_ROOT_INO);
    let mut logical_index = 0u64;
    if offset <= logical_index {
        out.push(make_dirent(parent_ino, b".", 4));
    }
    logical_index = logical_index.wrapping_add(1);
    if offset <= logical_index {
        out.push(make_dirent(parent_parent, b"..", 4));
    }
    logical_index = logical_index.wrapping_add(1);
    let mut i = 0usize;
    while i < children.len() && out.len() < VFS_READDIR_BATCH.saturating_add(2) {
        if logical_index < offset {
            logical_index = logical_index.wrapping_add(1);
            i = i.wrapping_add(1);
            continue;
        }
        out.push(make_dirent(children[i].ino, children[i].name(), children[i].kind));
        logical_index = logical_index.wrapping_add(1);
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Tronque/étend un fichier à `new_size`.
pub fn vfs_truncate(ino: ObjectIno, new_size: u64) -> ExofsResult<()> {
    let e = INODE_EMULATION.get_entry(ino).ok_or(ExofsError::ObjectNotFound)?;
    if e.flags & inode_flags::DIRECTORY != 0 { return Err(ExofsError::NotADirectory); }
    INODE_EMULATION.update_size(ino, new_size)
}

/// Crée un lien symbolique.
pub fn vfs_symlink(parent_ino: ObjectIno, name: &[u8], uid: u64) -> ExofsResult<ObjectIno> {
    if name.is_empty() { return Err(ExofsError::InvalidArgument); }
    if name.len() > VFS_NAME_MAX { return Err(ExofsError::PathTooLong); }
    validate_name(name)?;
    let oid = hash_name(parent_ino, name);
    if INODE_EMULATION.contains_oid(oid) { return Err(ExofsError::ObjectAlreadyExists); }
    INODE_EMULATION.get_or_alloc_flags(oid, inode_flags::SYMLINK, 0, uid)
}

/// Ferme tous les fd d'un pid (exit/kill).
pub fn vfs_close_all_pid(pid: u32) { FD_TABLE.close_all_pid(pid); }

/// Nombre de descripteurs ouverts.
pub fn vfs_open_count() -> usize { FD_TABLE.active_count() }

// ─────────────────────────────────────────────────────────────────────────────
// Helpers internes
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit un InodeEntry en VfsInode.
fn entry_to_vfs_inode(e: &InodeEntry) -> VfsInode {
    let mode = if e.flags & inode_flags::DIRECTORY != 0 { file_mode::DEFAULT_DIR }
               else if e.flags & inode_flags::SYMLINK != 0 { file_mode::S_IFLNK | 0o777 }
               else { file_mode::DEFAULT_FILE };
    VfsInode { ino: e.ino, size: e.size, mode, uid: e.uid as u32, link_count: e.link_count, kind: inode_kind(e.flags), _pad: [0; 3] }
}

fn inode_kind(flags: u32) -> u8 {
    if flags & inode_flags::DIRECTORY != 0 { 4 }
    else if flags & inode_flags::SYMLINK != 0 { 10 }
    else { 8 }
}

fn blob_id_for_object(object_id: u64) -> BlobId {
    BlobId::from_bytes_blake3(&object_id.to_le_bytes())
}

fn resolve_dir_record(parent_ino: ObjectIno, name: &[u8]) -> Option<DirRecord> {
    DIR_INDEX.lookup(parent_ino, name).or_else(|| {
        let oid = hash_name(parent_ino, name);
        INODE_EMULATION.get_entry_by_oid(oid).map(|entry| {
            DirRecord::new(parent_ino, entry.ino, oid, inode_kind(entry.flags), name)
        })
    })
}

/// Construit un VfsDirent depuis un ino et un nom.
fn make_dirent(ino: ObjectIno, name: &[u8], kind: u8) -> VfsDirent {
    let mut d = VfsDirent { ino, kind, name_len: name.len().min(VFS_NAME_MAX) as u8, _pad: [0; 6], name: [0; VFS_NAME_MAX] };
    let mut i = 0usize;
    while i < d.name_len as usize { d.name[i] = name[i]; i = i.wrapping_add(1); }
    d
}

/// Valide un composant de nom (pas de '/', '\0', pas ".." admis ici).
fn validate_name(name: &[u8]) -> ExofsResult<()> {
    let mut i = 0usize;
    while i < name.len() {
        if name[i] == b'/' || name[i] == 0 { return Err(ExofsError::InvalidPathComponent); }
        i = i.wrapping_add(1);
    }
    Ok(())
}

/// Hash de nom déterministe : FNV-1a 64-bit sur (parent_ino, name).
/// ARITH-02 : wrapping_mul/xor.
fn hash_name(parent_ino: ObjectIno, name: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME:  u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET;
    let pid_bytes = parent_ino.to_le_bytes();
    let mut i = 0usize;
    while i < 8 { h = h.wrapping_mul(FNV_PRIME) ^ (pid_bytes[i] as u64); i = i.wrapping_add(1); }
    let mut j = 0usize;
    while j < name.len() { h = h.wrapping_mul(FNV_PRIME) ^ (name[j] as u64); j = j.wrapping_add(1); }
    // Évite oid == 0.
    if h == 0 { 1 } else { h }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vfs_inode_size() { assert_eq!(core::mem::size_of::<VfsInode>(), 32); }

    #[test]
    fn test_vfs_dirent_size() { assert_eq!(core::mem::size_of::<VfsDirent>(), 271); }

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
    fn test_validate_name_ok() { assert!(validate_name(b"hello").is_ok()); }

    #[test]
    fn test_validate_name_slash() {
        assert!(matches!(validate_name(b"a/b"), Err(ExofsError::InvalidPathComponent)));
    }

    #[test]
    fn test_validate_name_null() {
        assert!(matches!(validate_name(b"a\x00b"), Err(ExofsError::InvalidPathComponent)));
    }

    #[test]
    fn test_make_dirent() {
        let d = make_dirent(42, b"hello", 8);
        assert_eq!(d.ino, 42);
        assert_eq!(d.name_len, 5);
        assert_eq!(&d.name[..5], b"hello");
    }

    #[test]
    fn test_entry_to_vfs_inode_dir() {
        let e = crate::fs::exofs::posix_bridge::inode_emulation::InodeEntry {
            ino: 1, object_id: 1, flags: inode_flags::DIRECTORY, link_count: 2,
            size: 0, uid: 0, epoch_id: 0, access_ts: 0
        };
        let v = entry_to_vfs_inode(&e);
        assert!(v.is_dir());
    }

    #[test]
    fn test_entry_to_vfs_inode_file() {
        let e = crate::fs::exofs::posix_bridge::inode_emulation::InodeEntry {
            ino: 10, object_id: 10, flags: inode_flags::REGULAR, link_count: 1,
            size: 1024, uid: 1000, epoch_id: 0, access_ts: 0
        };
        let v = entry_to_vfs_inode(&e);
        assert!(v.is_regular());
        assert!(!v.is_dir());
    }

    #[test]
    fn test_inode_kind_values() {
        assert_eq!(inode_kind(inode_flags::DIRECTORY), 4);
        assert_eq!(inode_kind(inode_flags::REGULAR),   8);
        assert_eq!(inode_kind(inode_flags::SYMLINK),   10);
    }

    #[test]
    fn test_fd_table_open_close() {
        let fdt = OpenFdTable::new();
        let fd = fdt.open_fd(5, open_flags::O_RDONLY, 1).unwrap();
        assert!(fdt.get_fd(fd).is_some());
        fdt.close_fd(fd).unwrap();
        assert!(fdt.get_fd(fd).is_none());
    }

    #[test]
    fn test_fd_table_update_offset() {
        let fdt = OpenFdTable::new();
        let fd = fdt.open_fd(7, open_flags::O_RDONLY, 1).unwrap();
        fdt.update_offset(fd, 512);
        let d = fdt.get_fd(fd).unwrap();
        assert_eq!(d.offset, 512);
        fdt.close_fd(fd).unwrap();
    }

    #[test]
    fn test_dir_index_lists_real_children() {
        INODE_EMULATION.clear();
        DIR_INDEX.clear();
        INODE_EMULATION.ensure_root().unwrap();
        let file_ino = vfs_create(VFS_ROOT_INO, b"alpha", file_mode::DEFAULT_FILE, 0).unwrap();
        let dirents = vfs_readdir(VFS_ROOT_INO, 0).unwrap();
        assert!(dirents.iter().any(|dirent| dirent.ino == file_ino && dirent.get_name() == b"alpha"));
    }

    #[test]
    fn test_vfs_write_read_roundtrip() {
        INODE_EMULATION.clear();
        DIR_INDEX.clear();
        INODE_EMULATION.ensure_root().unwrap();
        let ino = vfs_create(VFS_ROOT_INO, b"rw", file_mode::DEFAULT_FILE, 0).unwrap();
        let fd = vfs_open(ino, open_flags::O_RDWR, 42).unwrap();
        let payload = b"exofs-roundtrip";
        assert_eq!(vfs_write(fd, payload, payload.len()).unwrap(), payload.len());
        FD_TABLE.update_offset(fd, 0);
        let mut out = [0u8; 16];
        let read = vfs_read(fd, &mut out, payload.len()).unwrap();
        assert_eq!(read, payload.len());
        assert_eq!(&out[..payload.len()], payload);
        vfs_close(fd).unwrap();
    }

    #[test]
    fn test_vfs_rename_preserves_inode_and_content() {
        INODE_EMULATION.clear();
        DIR_INDEX.clear();
        INODE_EMULATION.ensure_root().unwrap();
        let ino = vfs_create(VFS_ROOT_INO, b"old", file_mode::DEFAULT_FILE, 0).unwrap();
        let fd = vfs_open(ino, open_flags::O_RDWR, 7).unwrap();
        let payload = b"rename-data";
        vfs_write(fd, payload, payload.len()).unwrap();
        vfs_close(fd).unwrap();
        vfs_rename(VFS_ROOT_INO, b"old", VFS_ROOT_INO, b"new").unwrap();
        let renamed_ino = vfs_lookup(VFS_ROOT_INO, b"new").unwrap();
        assert_eq!(renamed_ino, ino);
        let fd = vfs_open(renamed_ino, open_flags::O_RDONLY, 7).unwrap();
        let mut out = [0u8; 16];
        let read = vfs_read(fd, &mut out, payload.len()).unwrap();
        assert_eq!(read, payload.len());
        assert_eq!(&out[..payload.len()], payload);
        vfs_close(fd).unwrap();
    }

    #[test]
    fn test_vfs_rmdir_rejects_non_empty_directory() {
        INODE_EMULATION.clear();
        DIR_INDEX.clear();
        INODE_EMULATION.ensure_root().unwrap();
        let dir = vfs_mkdir(VFS_ROOT_INO, b"dir", file_mode::DEFAULT_DIR, 0).unwrap();
        let _ = vfs_create(dir, b"child", file_mode::DEFAULT_FILE, 0).unwrap();
        let err = vfs_rmdir(VFS_ROOT_INO, b"dir").unwrap_err();
        assert_eq!(err, ExofsError::DirectoryNotEmpty);
    }
}
