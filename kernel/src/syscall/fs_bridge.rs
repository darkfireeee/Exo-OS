// syscall/fs_bridge.rs — Interface de synchronisation syscall ↔ fs/
//
// Ce module définit le contrat entre la couche syscall et le module fs/.
// Lorsque `pub mod fs;` sera activé dans lib.rs, les stubs ci-dessous seront
// remplacés par des appels réels vers `crate::fs::*`.
//
// ARCHITECTURE :
//   syscall/table.rs            → appelle les fonctions de ce module
//   syscall/fs_bridge.rs        → dispatch vers crate::fs (quand activé)
//   crate::fs (couche 4)        → implémentation VFS réelle
//
// ACTIVATION :
//   1. Activer `pub mod fs;` dans kernel/src/lib.rs
//   2. Remplacer chaque `Err(FsBridgeError::NotReady)` par le vrai appel fs/.
//   3. Supprimer la garde `FS_READY` (ou la laisser pour le mode dégradé).
//
// RÈGLE FS-BRIDGE-01 : Ce module ne doit JAMAIS importer fs/ directement.
//   Il utilise uniquement des types primitifs (u64, u32, &[u8], i64).
// RÈGLE FS-BRIDGE-02 : Toutes les fonctions retournent `Result<i64, FsBridgeError>`.
//   La valeur `Ok(n)` est le code de retour POSIX (octets, 0 pour succès...).
//   La valeur `Err(...)` est convertie en errno par le syscall handler.
// RÈGLE FS-BRIDGE-03 : `FS_READY.load()` doit retourner `true` avant tout appel.

use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::{BlobId, ExofsError, ObjectId};
use crate::fs::exofs::path::path_component::{PathComponent, PathComponentBuf};
use crate::fs::exofs::path::path_index::{
    PathIndex, PathIndexHeader, PATH_INDEX_MAGIC, PATH_INDEX_VERSION,
};
use crate::fs::exofs::path::symlink::{
    invalidate_symlink, is_valid_symlink_target, register_symlink, SYMLINK_MAX_DEPTH,
};
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::syscall::validation::{copy_from_user, copy_to_user, write_user_typed};

// ─────────────────────────────────────────────────────────────────────────────
// État du bridge — atomique pour thread-safety
// ─────────────────────────────────────────────────────────────────────────────

/// Indique si le sous-système fs/ est initialisé et prêt.
/// Mis à `true` par `fs_bridge_init()` lors du boot.
static FS_READY: AtomicBool = AtomicBool::new(false);

/// Initialise le bridge fs.
/// À appeler depuis la séquence de boot après `fs::init()`.
///
/// # Safety
/// Doit être appelé exactement une fois depuis le BSP.
pub unsafe fn fs_bridge_init() {
    FS_READY.store(true, Ordering::Release);
}

/// Retourne `true` si le sous-système fs/ est prêt.
#[inline(always)]
pub fn is_fs_ready() -> bool {
    FS_READY.load(Ordering::Acquire)
}

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs du bridge
// ─────────────────────────────────────────────────────────────────────────────

/// Erreurs retournées par le bridge fs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsBridgeError {
    /// fs/ non encore initialisé (FS_READY = false).
    NotReady,
    /// Descripteur de fichier invalide.
    BadFd,
    /// Chemin invalide ou trop long.
    BadPath,
    /// Fichier non trouvé.
    NotFound,
    /// Permission refusée.
    PermDenied,
    /// Pointeur userspace invalide (EFAULT).
    Fault,
    /// Argument invalide.
    Invalid,
    /// Espace disque insuffisant.
    NoSpace,
    /// Existe déjà.
    Exists,
    /// Pas un répertoire.
    NotDir,
    /// Est un répertoire (pas un fichier ordinaire).
    IsDir,
    /// Répertoire non vide.
    NotEmpty,
    /// Boucle de symlinks.
    Loop,
    /// Erreur I/O non spécifiée.
    Io,
}

impl FsBridgeError {
    /// Convertit en errno POSIX (valeur négative).
    pub fn to_errno(self) -> i64 {
        match self {
            FsBridgeError::NotReady => -38,   // ENOSYS
            FsBridgeError::BadFd => -9,       // EBADF
            FsBridgeError::BadPath => -22,    // EINVAL
            FsBridgeError::NotFound => -2,    // ENOENT
            FsBridgeError::PermDenied => -13, // EACCES
            FsBridgeError::Fault => -14,      // EFAULT
            FsBridgeError::Invalid => -22,    // EINVAL
            FsBridgeError::NoSpace => -28,    // ENOSPC
            FsBridgeError::Exists => -17,     // EEXIST
            FsBridgeError::NotDir => -20,     // ENOTDIR
            FsBridgeError::IsDir => -21,      // EISDIR
            FsBridgeError::NotEmpty => -39,   // ENOTEMPTY
            FsBridgeError::Loop => -40,       // ELOOP
            FsBridgeError::Io => -5,          // EIO
        }
    }
}

const SEEK_SET: u32 = 0;
const SEEK_CUR: u32 = 1;
const SEEK_END: u32 = 2;
const AT_FDCWD: i32 = -100;
const F_DUPFD: u32 = 0;
const F_GETFD: u32 = 1;
const F_SETFD: u32 = 2;
const F_GETFL: u32 = 3;
const F_SETFL: u32 = 4;
const STAT_BLOCK_SIZE: i64 = 4096;
const STAT_MODE_DIR: u32 = 0o040000 | 0o755;
const STAT_MODE_FILE: u32 = 0o100000 | 0o644;
const STAT_MODE_SYMLINK: u32 = 0o120000 | 0o777;
const PATH_INDEX_KIND_DIR: u8 = 0;
const PATH_INDEX_KIND_FILE: u8 = 1;
const PATH_INDEX_KIND_SYMLINK: u8 = 2;
const DT_UNKNOWN: u8 = 0;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;
#[cfg(test)]
const STAT_MODE_MASK: u32 = 0o170000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxTimespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: u32,
    st_uid: u32,
    st_gid: u32,
    __pad0: u32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: i64,
    st_blocks: i64,
    st_atim: LinuxTimespec,
    st_mtim: LinuxTimespec,
    st_ctim: LinuxTimespec,
    __unused: [i64; 3],
}

const _: () = assert!(size_of::<LinuxStat>() == 144);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxDirent64 {
    d_ino: u64,
    d_off: i64,
    d_reclen: u16,
    d_type: u8,
}

const DIRENT64_HEADER_SIZE: usize = size_of::<LinuxDirent64>();

#[inline]
fn exofs_to_bridge_error(err: ExofsError) -> FsBridgeError {
    match err {
        ExofsError::ObjectNotFound | ExofsError::BlobNotFound | ExofsError::NotFound => {
            FsBridgeError::NotFound
        }
        ExofsError::PermissionDenied => FsBridgeError::PermDenied,
        ExofsError::InvalidArgument
        | ExofsError::InvalidPathComponent
        | ExofsError::InvalidState
        | ExofsError::InvalidSize
        | ExofsError::PathTooLong
        | ExofsError::OffsetOverflow
        | ExofsError::Overflow
        | ExofsError::Underflow => FsBridgeError::Invalid,
        ExofsError::NoSpace => FsBridgeError::NoSpace,
        ExofsError::ObjectAlreadyExists | ExofsError::AlreadyExists => FsBridgeError::Exists,
        ExofsError::NotADirectory => FsBridgeError::NotDir,
        ExofsError::DirectoryNotEmpty => FsBridgeError::NotEmpty,
        ExofsError::TooManySymlinks => FsBridgeError::Loop,
        ExofsError::IoError
        | ExofsError::IoFailed
        | ExofsError::NvmeFlushFailed
        | ExofsError::PartialWrite
        | ExofsError::ShortWrite => FsBridgeError::Io,
        _ => FsBridgeError::Io,
    }
}

#[inline]
fn validate_path(path: &[u8]) -> Result<(), FsBridgeError> {
    if path.is_empty() || path.len() > 4096 || path.contains(&0) {
        return Err(FsBridgeError::BadPath);
    }
    Ok(())
}

#[inline]
fn normalize_path_buf(path: &[u8]) -> Result<PathComponentBuf, FsBridgeError> {
    validate_path(path)?;
    let mut buf = PathComponentBuf::from_path(path).map_err(exofs_to_bridge_error)?;
    buf.normalize().map_err(exofs_to_bridge_error)?;
    Ok(buf)
}

#[inline]
fn path_buf_to_bytes(buf: &PathComponentBuf) -> Result<Vec<u8>, FsBridgeError> {
    buf.to_bytes().map_err(exofs_to_bridge_error)
}

#[inline]
fn normalized_path_bytes(path: &[u8]) -> Result<Vec<u8>, FsBridgeError> {
    let buf = normalize_path_buf(path)?;
    path_buf_to_bytes(&buf)
}

#[inline]
fn split_parent_and_leaf(path: &[u8]) -> Result<(Vec<u8>, PathComponent), FsBridgeError> {
    let buf = normalize_path_buf(path)?;
    let leaf = buf.last().cloned().ok_or(FsBridgeError::BadPath)?;
    let mut parent_buf = PathComponentBuf::new();
    for comp in buf.parent() {
        parent_buf
            .push(comp.clone())
            .map_err(exofs_to_bridge_error)?;
    }
    let parent_path = path_buf_to_bytes(&parent_buf)?;
    Ok((parent_path, leaf))
}

#[inline]
fn blob_id_for_path(path: &[u8]) -> Result<BlobId, FsBridgeError> {
    validate_path(path)?;
    Ok(BlobId::from_bytes_blake3(path))
}

#[inline]
fn blob_len(blob_id: &BlobId) -> usize {
    BLOB_CACHE.get(blob_id).map(|data| data.len()).unwrap_or(0)
}

#[inline]
fn ensure_blob_exists(blob_id: BlobId) -> Result<(), FsBridgeError> {
    if BLOB_CACHE.contains(&blob_id) {
        return Ok(());
    }
    BLOB_CACHE
        .insert(blob_id, Vec::new())
        .map_err(exofs_to_bridge_error)
}

#[inline]
fn snapshot_blob(blob_id: &BlobId) -> Result<Vec<u8>, FsBridgeError> {
    match BLOB_CACHE.get(blob_id) {
        Some(data) => Ok(data.into_vec()),
        None => Err(FsBridgeError::NotFound),
    }
}

#[inline]
fn blob_id_to_object_id(blob_id: BlobId) -> ObjectId {
    ObjectId(*blob_id.as_bytes())
}

#[inline]
fn object_id_to_blob_id(object_id: ObjectId) -> BlobId {
    BlobId(*object_id.as_bytes())
}

#[inline]
fn ensure_root_directory() -> Result<(), FsBridgeError> {
    let root_blob = blob_id_for_path(b"/")?;
    if BLOB_CACHE.contains(&root_blob) {
        return Ok(());
    }

    let dir_index = PathIndex::new_with_key(ObjectId::default(), directory_mount_key());
    let bytes = dir_index.serialize().map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(root_blob, bytes)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&root_blob);
    Ok(())
}

#[inline]
fn load_path_index(path: &[u8]) -> Result<PathIndex, FsBridgeError> {
    let blob_id = blob_id_for_path(path)?;
    let data = snapshot_blob(&blob_id)?;
    if !blob_is_directory(&data) {
        return Err(FsBridgeError::NotDir);
    }
    PathIndex::from_bytes(&data).map_err(exofs_to_bridge_error)
}

#[inline]
fn store_path_index(path: &[u8], index: &PathIndex) -> Result<(), FsBridgeError> {
    let blob_id = blob_id_for_path(path)?;
    let bytes = index.serialize().map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(blob_id, bytes)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    Ok(())
}

#[inline]
fn ensure_directory_exists(path: &[u8]) -> Result<(), FsBridgeError> {
    if path == b"/" {
        return ensure_root_directory();
    }

    let blob_id = blob_id_for_path(path)?;
    let data = snapshot_blob(&blob_id)?;
    if blob_is_directory(&data) {
        Ok(())
    } else {
        Err(FsBridgeError::NotDir)
    }
}

#[inline]
fn dirent_type_from_kind(kind: u8) -> u8 {
    match kind {
        PATH_INDEX_KIND_DIR => DT_DIR,
        PATH_INDEX_KIND_FILE => DT_REG,
        PATH_INDEX_KIND_SYMLINK => DT_LNK,
        _ => DT_UNKNOWN,
    }
}

#[inline]
fn inode_from_object_id(object_id: &ObjectId) -> u64 {
    let bytes = object_id.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

#[inline]
fn upsert_parent_entry(
    parent_path: &[u8],
    leaf: &PathComponent,
    child_blob_id: BlobId,
    kind: u8,
) -> Result<(), FsBridgeError> {
    ensure_directory_exists(parent_path)?;
    let mut index = load_path_index(parent_path)?;
    if index.lookup(leaf).is_some() {
        index.remove(leaf).map_err(exofs_to_bridge_error)?;
    }
    index
        .insert(leaf, blob_id_to_object_id(child_blob_id), kind)
        .map_err(exofs_to_bridge_error)?;
    store_path_index(parent_path, &index)
}

#[inline]
fn remove_parent_entry(parent_path: &[u8], leaf: &PathComponent) -> Result<(), FsBridgeError> {
    ensure_directory_exists(parent_path)?;
    let mut index = load_path_index(parent_path)?;
    index.remove(leaf).map_err(exofs_to_bridge_error)?;
    store_path_index(parent_path, &index)
}

#[inline]
fn directory_mount_key() -> [u8; 16] {
    #[cfg(target_os = "none")]
    {
        crate::fs::exofs::path::path_index::mount_secret_key()
    }

    #[cfg(not(target_os = "none"))]
    {
        [0xA5, 0, 0, 0, 0, 0, 0, 0x5A, 0, 0, 0, 0, 0, 0, 0, 0xC3]
    }
}

#[inline]
fn path_index_entry_count(data: &[u8]) -> Option<u32> {
    if data.len() < size_of::<PathIndexHeader>() {
        return None;
    }

    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != PATH_INDEX_MAGIC {
        return None;
    }

    let version = u16::from_le_bytes([data[4], data[5]]);
    if version != PATH_INDEX_VERSION {
        return None;
    }

    Some(u32::from_le_bytes([data[40], data[41], data[42], data[43]]))
}

#[inline]
fn blob_is_directory(data: &[u8]) -> bool {
    path_index_entry_count(data).is_some()
}

#[inline]
fn stat_mode_for_kind(kind: u8, data: &[u8]) -> u32 {
    match kind {
        PATH_INDEX_KIND_DIR => STAT_MODE_DIR,
        PATH_INDEX_KIND_SYMLINK => STAT_MODE_SYMLINK,
        _ => {
            if blob_is_directory(data) {
                STAT_MODE_DIR
            } else {
                STAT_MODE_FILE
            }
        }
    }
}

#[inline]
fn inode_from_blob_id(blob_id: &BlobId) -> u64 {
    let bytes = blob_id.as_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

#[inline]
fn blocks_for_size(size: u64) -> i64 {
    size.saturating_add(511).saturating_div(512) as i64
}

#[inline]
fn linux_stat_for_blob(blob_id: BlobId, data: &[u8], owner_uid: u32, kind: u8) -> LinuxStat {
    let size = data.len() as u64;
    LinuxStat {
        st_dev: 0,
        st_ino: inode_from_blob_id(&blob_id),
        st_nlink: 1,
        st_mode: stat_mode_for_kind(kind, data),
        st_uid: owner_uid,
        st_gid: owner_uid,
        __pad0: 0,
        st_rdev: 0,
        st_size: size as i64,
        st_blksize: STAT_BLOCK_SIZE,
        st_blocks: blocks_for_size(size),
        st_atim: LinuxTimespec::default(),
        st_mtim: LinuxTimespec::default(),
        st_ctim: LinuxTimespec::default(),
        __unused: [0; 3],
    }
}

#[inline]
fn path_entry(path: &[u8]) -> Result<(BlobId, u8), FsBridgeError> {
    ensure_root_directory()?;
    if path == b"/" {
        return Ok((blob_id_for_path(path)?, PATH_INDEX_KIND_DIR));
    }
    let (parent_path, leaf) = split_parent_and_leaf(path)?;
    let index = load_path_index(&parent_path)?;
    let (object_id, kind) = index.lookup(&leaf).ok_or(FsBridgeError::NotFound)?;
    Ok((object_id_to_blob_id(object_id), kind))
}

fn resolve_path_with_symlinks(
    path: &[u8],
    follow_last: bool,
    allow_missing_final: bool,
) -> Result<Vec<u8>, FsBridgeError> {
    let mut pending = normalize_path_buf(path)?;
    let mut resolved = PathComponentBuf::new();
    let mut depth = 0usize;
    let mut idx = 0usize;

    ensure_root_directory()?;

    while idx < pending.len() {
        let comp = pending.as_slice()[idx].clone();
        let parent_path = path_buf_to_bytes(&resolved)?;
        ensure_directory_exists(&parent_path)?;

        let index = load_path_index(&parent_path)?;
        let (object_id, kind) = match index.lookup(&comp) {
            Some(entry) => entry,
            None => {
                let is_last = idx + 1 == pending.len();
                if !allow_missing_final || !is_last {
                    return Err(FsBridgeError::NotFound);
                }
                resolved.push(comp).map_err(exofs_to_bridge_error)?;
                return path_buf_to_bytes(&resolved);
            }
        };

        let is_last = idx + 1 == pending.len();
        if kind == PATH_INDEX_KIND_SYMLINK && (!is_last || follow_last) {
            depth = depth.saturating_add(1);
            if depth > SYMLINK_MAX_DEPTH {
                return Err(FsBridgeError::Loop);
            }

            let raw_target = snapshot_blob(&object_id_to_blob_id(object_id))?;
            if !is_valid_symlink_target(&raw_target) {
                return Err(FsBridgeError::Invalid);
            }

            let mut next = if raw_target.starts_with(b"/") {
                normalize_path_buf(&raw_target)?
            } else {
                let mut joined = resolved.clone();
                let target_buf =
                    PathComponentBuf::from_path(&raw_target).map_err(exofs_to_bridge_error)?;
                joined
                    .extend_from(&target_buf)
                    .map_err(exofs_to_bridge_error)?;
                joined
            };

            let mut rem_idx = idx + 1;
            while rem_idx < pending.len() {
                next.push(pending.as_slice()[rem_idx].clone())
                    .map_err(exofs_to_bridge_error)?;
                rem_idx += 1;
            }
            next.normalize().map_err(exofs_to_bridge_error)?;
            pending = next;
            resolved = PathComponentBuf::new();
            idx = 0;
            continue;
        }

        resolved.push(comp).map_err(exofs_to_bridge_error)?;
        idx += 1;
    }

    path_buf_to_bytes(&resolved)
}

/// `read(fd, buf, count)` → octets lus.
/// PONT : `crate::fs::vfs::read(fd, buf_slice)` — activé quand `pub mod fs;`
#[inline]
pub fn fs_read(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }

    let entry = OBJECT_TABLE.get(fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_read() {
        return Err(FsBridgeError::PermDenied);
    }

    let data = match BLOB_CACHE.get(&entry.blob_id) {
        Some(bytes) => bytes,
        None if entry.size == 0 => return Ok(0),
        None => return Err(FsBridgeError::NotFound),
    };

    let start = entry.cursor as usize;
    if start >= data.len() {
        return Ok(0);
    }

    let read_len = count.min(data.len() - start);
    copy_to_user(
        buf_ptr as *mut u8,
        data[start..start + read_len].as_ptr(),
        read_len,
    )
    .map_err(|_| FsBridgeError::Fault)?;
    OBJECT_TABLE
        .advance_cursor(fd, read_len as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(read_len as i64)
}

/// `write(fd, buf, count)` → octets écrits.
#[inline]
pub fn fs_write(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if buf_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    if count == 0 {
        return Ok(0);
    }

    let entry = OBJECT_TABLE.get(fd).map_err(exofs_to_bridge_error)?;
    if !entry.can_write() {
        return Err(FsBridgeError::PermDenied);
    }

    let mut input = Vec::new();
    input.resize(count, 0);
    copy_from_user(input.as_mut_ptr(), buf_ptr as *const u8, count)
        .map_err(|_| FsBridgeError::Fault)?;

    let mut data = match BLOB_CACHE.get(&entry.blob_id) {
        Some(bytes) => bytes.into_vec(),
        None => Vec::new(),
    };
    let start = if entry.flags & open_flags::O_APPEND != 0 {
        data.len()
    } else {
        entry.cursor as usize
    };
    let end = start.checked_add(count).ok_or(FsBridgeError::NoSpace)?;
    if data.len() < end {
        data.resize(end, 0);
    }
    data[start..end].copy_from_slice(&input);

    BLOB_CACHE
        .insert(entry.blob_id, data)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&entry.blob_id);
    OBJECT_TABLE
        .set_cursor(fd, end as u64)
        .map_err(exofs_to_bridge_error)?;
    OBJECT_TABLE
        .set_size(fd, end as u64)
        .map_err(exofs_to_bridge_error)?;
    let _ = pid;
    Ok(count as i64)
}

/// `open(path, flags, mode)` → fd.
#[inline]
pub fn fs_open(path: &[u8], flags: u32, mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = mode;
    if !open_flags::validate(flags) {
        return Err(FsBridgeError::Invalid);
    }

    let normalized_path = resolve_path_with_symlinks(path, true, true)?;
    ensure_root_directory()?;
    let existing_entry = path_entry(&normalized_path).ok();
    let blob_id = blob_id_for_path(&normalized_path)?;
    let exists = existing_entry.is_some();

    if !exists && flags & open_flags::O_CREAT == 0 {
        return Err(FsBridgeError::NotFound);
    }
    if exists && (flags & open_flags::O_CREAT != 0) && (flags & open_flags::O_EXCL != 0) {
        return Err(FsBridgeError::Exists);
    }
    if !exists {
        let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
        ensure_directory_exists(&parent_path)?;
        ensure_blob_exists(blob_id)?;
        upsert_parent_entry(&parent_path, &leaf, blob_id, PATH_INDEX_KIND_FILE)?;
    }
    if flags & open_flags::O_TRUNC != 0 {
        if !open_flags::can_write(flags) {
            return Err(FsBridgeError::Invalid);
        }
        let existing = snapshot_blob(&blob_id)?;
        if blob_is_directory(&existing) {
            return Err(FsBridgeError::IsDir);
        }
        BLOB_CACHE
            .insert(blob_id, Vec::new())
            .map_err(exofs_to_bridge_error)?;
        let _ = BLOB_CACHE.mark_dirty(&blob_id);
    }

    let size = blob_len(&blob_id) as u64;
    let fd = OBJECT_TABLE
        .open(blob_id, flags, size, 0, pid as u64)
        .map_err(exofs_to_bridge_error)?;
    if flags & open_flags::O_APPEND != 0 {
        OBJECT_TABLE
            .set_cursor(fd, size)
            .map_err(exofs_to_bridge_error)?;
    }
    Ok(fd as i64)
}

/// `close(fd)`.
#[inline]
pub fn fs_close(fd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if OBJECT_TABLE.close(fd) {
        Ok(0)
    } else {
        Err(FsBridgeError::BadFd)
    }
}

/// `lseek(fd, offset, whence)` → nouvelle position.
#[inline]
pub fn fs_lseek(fd: u32, offset: i64, whence: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let entry = OBJECT_TABLE.get(fd).map_err(exofs_to_bridge_error)?;
    let base = match whence {
        SEEK_SET => 0i64,
        SEEK_CUR => entry.cursor as i64,
        SEEK_END => snapshot_blob(&entry.blob_id)
            .map(|data| data.len() as i64)
            .unwrap_or(entry.size as i64),
        _ => return Err(FsBridgeError::Invalid),
    };
    let new_pos = base.checked_add(offset).ok_or(FsBridgeError::Invalid)?;
    if new_pos < 0 {
        return Err(FsBridgeError::Invalid);
    }
    OBJECT_TABLE
        .set_cursor(fd, new_pos as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(new_pos)
}

/// `stat(path, stat_ptr)`.
#[inline]
pub fn fs_stat(path: &[u8], stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if stat_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, true, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let data = snapshot_blob(&blob_id)?;
    let stat = linux_stat_for_blob(blob_id, &data, pid, kind);
    write_user_typed(stat_ptr, stat).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `lstat(path, stat_ptr)` — ne suit pas le symlink terminal.
#[inline]
pub fn fs_lstat(path: &[u8], stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if stat_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let data = snapshot_blob(&blob_id)?;
    let stat = linux_stat_for_blob(blob_id, &data, pid, kind);
    write_user_typed(stat_ptr, stat).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `fstat(fd, stat_ptr)`.
#[inline]
pub fn fs_fstat(fd: u32, stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if stat_ptr == 0 {
        return Err(FsBridgeError::Fault);
    }
    let entry = OBJECT_TABLE.get(fd).map_err(exofs_to_bridge_error)?;
    let data = snapshot_blob(&entry.blob_id)?;
    let owner_uid = if entry.owner_uid == 0 {
        pid
    } else {
        entry.owner_uid as u32
    };
    let kind = if blob_is_directory(&data) {
        PATH_INDEX_KIND_DIR
    } else {
        PATH_INDEX_KIND_FILE
    };
    let stat = linux_stat_for_blob(entry.blob_id, &data, owner_uid, kind);
    write_user_typed(stat_ptr, stat).map_err(|_| FsBridgeError::Fault)?;
    Ok(0)
}

/// `openat(dirfd, path, flags, mode)`.
#[inline]
pub fn fs_openat(
    dirfd: i32,
    path: &[u8],
    flags: u32,
    mode: u32,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    // ExoFS ne maintient pas encore de namespace de répertoires ouverts.
    // Cas supportés sans ambiguïté:
    // - AT_FDCWD + chemin relatif/absolu
    // - chemin absolu, quel que soit dirfd (sémantique POSIX)
    if dirfd == AT_FDCWD || path.starts_with(b"/") {
        return fs_open(path, flags, mode, pid);
    }
    Err(FsBridgeError::Invalid)
}

/// `symlink(target, linkpath)`.
#[inline]
pub fn fs_symlink(target: &[u8], linkpath: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if !is_valid_symlink_target(target) {
        return Err(FsBridgeError::Invalid);
    }

    let normalized_link = resolve_path_with_symlinks(linkpath, false, true)?;
    if normalized_link == b"/" {
        return Err(FsBridgeError::BadPath);
    }
    if path_entry(&normalized_link).is_ok() {
        return Err(FsBridgeError::Exists);
    }

    let (parent_path, leaf) = split_parent_and_leaf(&normalized_link)?;
    ensure_directory_exists(&parent_path)?;
    let blob_id = blob_id_for_path(&normalized_link)?;
    BLOB_CACHE
        .insert(blob_id, target.to_vec())
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    register_symlink(&blob_id_to_object_id(blob_id), target).map_err(exofs_to_bridge_error)?;
    upsert_parent_entry(&parent_path, &leaf, blob_id, PATH_INDEX_KIND_SYMLINK)?;
    Ok(0)
}

/// `symlinkat(target, dirfd, linkpath)`.
#[inline]
pub fn fs_symlinkat(
    target: &[u8],
    dirfd: i32,
    linkpath: &[u8],
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if dirfd == AT_FDCWD || linkpath.starts_with(b"/") {
        return fs_symlink(target, linkpath, pid);
    }
    Err(FsBridgeError::Invalid)
}

/// `dup(oldfd)`.
#[inline]
pub fn fs_dup(oldfd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    OBJECT_TABLE
        .dup(oldfd)
        .map(|fd| fd as i64)
        .map_err(exofs_to_bridge_error)
}

/// `dup2(oldfd, newfd)`.
#[inline]
pub fn fs_dup2(oldfd: u32, newfd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    OBJECT_TABLE
        .dup2(oldfd, newfd)
        .map(|fd| fd as i64)
        .map_err(exofs_to_bridge_error)
}

/// `fcntl(fd, cmd, arg)`.
#[inline]
pub fn fs_fcntl(fd: u32, cmd: u32, arg: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    match cmd {
        F_DUPFD => OBJECT_TABLE
            .dup_from(fd, arg as u32)
            .map(|new_fd| new_fd as i64)
            .map_err(exofs_to_bridge_error),
        F_GETFD => Ok(0),
        F_SETFD => {
            if arg != 0 {
                return Err(FsBridgeError::Invalid);
            }
            Ok(0)
        }
        F_GETFL => OBJECT_TABLE
            .get(fd)
            .map(|entry| entry.flags as i64)
            .map_err(exofs_to_bridge_error),
        F_SETFL => {
            let supported = open_flags::O_APPEND as u64;
            if arg & !supported != 0 {
                return Err(FsBridgeError::Invalid);
            }
            OBJECT_TABLE
                .set_status_flags(fd, arg as u32)
                .map(|flags| flags as i64)
                .map_err(exofs_to_bridge_error)
        }
        _ => Err(FsBridgeError::Invalid),
    }
}

/// `mkdir(path, mode)`.
#[inline]
pub fn fs_mkdir(path: &[u8], mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = (mode, pid);
    let normalized_path = normalized_path_bytes(path)?;
    if normalized_path == b"/" {
        ensure_root_directory()?;
        return Ok(0);
    }
    ensure_root_directory()?;
    let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
    ensure_directory_exists(&parent_path)?;
    let blob_id = blob_id_for_path(&normalized_path)?;
    if BLOB_CACHE.contains(&blob_id) {
        return Err(FsBridgeError::Exists);
    }

    let parent_oid = if parent_path == b"/" {
        ObjectId::default()
    } else {
        blob_id_to_object_id(blob_id_for_path(&parent_path)?)
    };
    let dir_index = PathIndex::new_with_key(parent_oid, directory_mount_key());
    let bytes = dir_index.serialize().map_err(exofs_to_bridge_error)?;
    BLOB_CACHE
        .insert(blob_id, bytes)
        .map_err(exofs_to_bridge_error)?;
    let _ = BLOB_CACHE.mark_dirty(&blob_id);
    upsert_parent_entry(&parent_path, &leaf, blob_id, PATH_INDEX_KIND_DIR)?;
    Ok(0)
}

/// `rmdir(path)`.
#[inline]
pub fn fs_rmdir(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = normalized_path_bytes(path)?;
    if normalized_path == b"/" {
        return Err(FsBridgeError::PermDenied);
    }
    let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
    let blob_id = blob_id_for_path(&normalized_path)?;
    let data = snapshot_blob(&blob_id)?;
    let entry_count = path_index_entry_count(&data).ok_or(FsBridgeError::NotDir)?;
    if entry_count != 0 {
        return Err(FsBridgeError::NotEmpty);
    }
    if OBJECT_TABLE.open_count_for(&blob_id) != 0 {
        return Err(FsBridgeError::PermDenied);
    }
    BLOB_CACHE.invalidate(&blob_id);
    remove_parent_entry(&parent_path, &leaf)?;
    Ok(0)
}

/// `unlink(path)`.
#[inline]
pub fn fs_unlink(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    if normalized_path == b"/" {
        return Err(FsBridgeError::PermDenied);
    }
    let (parent_path, leaf) = split_parent_and_leaf(&normalized_path)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    let data = snapshot_blob(&blob_id)?;
    if kind == PATH_INDEX_KIND_DIR || blob_is_directory(&data) {
        return Err(FsBridgeError::IsDir);
    }
    if OBJECT_TABLE.open_count_for(&blob_id) != 0 {
        return Err(FsBridgeError::PermDenied);
    }
    if kind == PATH_INDEX_KIND_SYMLINK {
        invalidate_symlink(&blob_id_to_object_id(blob_id));
    }
    BLOB_CACHE.invalidate(&blob_id);
    remove_parent_entry(&parent_path, &leaf)?;
    Ok(0)
}

/// `getdents64(fd, dirp, count)`.
#[inline]
pub fn fs_getdents64(fd: u32, dirp: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if dirp == 0 {
        return Err(FsBridgeError::Fault);
    }
    if count < DIRENT64_HEADER_SIZE + 2 {
        return Err(FsBridgeError::Invalid);
    }

    let entry = OBJECT_TABLE.get(fd).map_err(exofs_to_bridge_error)?;
    let data = snapshot_blob(&entry.blob_id)?;
    if !blob_is_directory(&data) {
        return Err(FsBridgeError::NotDir);
    }

    let index = PathIndex::from_bytes(&data).map_err(exofs_to_bridge_error)?;
    let entries = index.entries();
    let start_idx = entry.cursor as usize;
    if start_idx >= entries.len() {
        return Ok(0);
    }

    let mut out = Vec::new();
    out.try_reserve(count).map_err(|_| FsBridgeError::NoSpace)?;
    let mut cursor = start_idx;
    while cursor < entries.len() {
        let entry_ref = &entries[cursor];
        let name = entry_ref.name_bytes();
        let raw_size = DIRENT64_HEADER_SIZE
            .checked_add(name.len())
            .and_then(|v| v.checked_add(1))
            .ok_or(FsBridgeError::NoSpace)?;
        let reclen = (raw_size + 7) & !7usize;
        if out.len().saturating_add(reclen) > count {
            break;
        }

        let header = LinuxDirent64 {
            d_ino: inode_from_object_id(&entry_ref.oid),
            d_off: (cursor + 1) as i64,
            d_reclen: reclen as u16,
            d_type: dirent_type_from_kind(entry_ref.kind),
        };

        // SAFETY: LinuxDirent64 est #[repr(C)] et POD.
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                &header as *const LinuxDirent64 as *const u8,
                DIRENT64_HEADER_SIZE,
            )
        };

        out.extend_from_slice(header_bytes);
        out.extend_from_slice(name);
        out.push(0);
        while out.len() % 8 != 0 {
            out.push(0);
        }
        cursor += 1;
    }

    if out.is_empty() {
        return Ok(0);
    }

    copy_to_user(dirp as *mut u8, out.as_ptr(), out.len()).map_err(|_| FsBridgeError::Fault)?;
    OBJECT_TABLE
        .set_cursor(fd, cursor as u64)
        .map_err(exofs_to_bridge_error)?;
    Ok(out.len() as i64)
}

/// `readlink(path, buf, bufsize)`.
#[inline]
pub fn fs_readlink(path: &[u8], buf: u64, bufsize: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    let _ = pid;
    if buf == 0 && bufsize != 0 {
        return Err(FsBridgeError::Fault);
    }
    let normalized_path = resolve_path_with_symlinks(path, false, false)?;
    let (blob_id, kind) = path_entry(&normalized_path)?;
    if kind != PATH_INDEX_KIND_SYMLINK {
        return Err(FsBridgeError::Invalid);
    }
    let target = snapshot_blob(&blob_id)?;
    if !is_valid_symlink_target(&target) {
        return Err(FsBridgeError::Invalid);
    }
    let copy_len = target.len().min(bufsize);
    if copy_len != 0 {
        copy_to_user(buf as *mut u8, target.as_ptr(), copy_len)
            .map_err(|_| FsBridgeError::Fault)?;
    }
    Ok(copy_len as i64)
}

/// `readlinkat(dirfd, path, buf, bufsize)`.
#[inline]
pub fn fs_readlinkat(
    dirfd: i32,
    path: &[u8],
    buf: u64,
    bufsize: usize,
    pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() {
        return Err(FsBridgeError::NotReady);
    }
    if dirfd == AT_FDCWD || path.starts_with(b"/") {
        return fs_readlink(path, buf, bufsize, pid);
    }
    Err(FsBridgeError::Invalid)
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper : conversion automatique pour les syscall handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit un `Result<i64, FsBridgeError>` en code de retour syscall.
#[inline(always)]
pub fn bridge_result(r: Result<i64, FsBridgeError>) -> i64 {
    match r {
        Ok(n) => n,
        Err(e) => e.to_errno(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_bridge() {
        unsafe {
            fs_bridge_init();
        }
        OBJECT_TABLE.reset_all();
        BLOB_CACHE.flush_all();
    }

    #[test]
    fn test_fs_bridge_open_write_read_lseek_roundtrip() {
        init_bridge();

        let path = b"/etc/exo/kernel.toml";
        let payload = *b"exo-kernel-config";
        let mut read_back = [0u8; 17];

        let fd = fs_open(path, open_flags::O_RDWR | open_flags::O_CREAT, 0, 7).unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 7).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_lseek(fd, 0, SEEK_SET, 7).unwrap(), 0);
        assert_eq!(
            fs_read(fd, read_back.as_mut_ptr() as u64, read_back.len(), 7).unwrap(),
            read_back.len() as i64
        );
        assert_eq!(&read_back, &payload);
        assert_eq!(fs_close(fd, 7).unwrap(), 0);
    }

    #[test]
    fn test_fs_bridge_roundtrip_stress() {
        init_bridge();

        for idx in 0..512u32 {
            let mut path = [0u8; 32];
            let prefix_len = 8;
            path[..prefix_len].copy_from_slice(b"/stress/");
            let suffix_len = write_u32_hex(&mut path[prefix_len..], idx);

            let write_len = (idx as usize % 48) + 1;
            let mut payload = [0u8; 64];
            for (off, byte) in payload[..write_len].iter_mut().enumerate() {
                *byte = idx.wrapping_add(off as u32) as u8;
            }

            let fd = fs_open(
                &path[..prefix_len + suffix_len],
                open_flags::O_RDWR | open_flags::O_CREAT | open_flags::O_TRUNC,
                0,
                11,
            )
            .unwrap() as u32;
            assert_eq!(
                fs_write(fd, payload.as_ptr() as u64, write_len, 11).unwrap(),
                write_len as i64
            );
            assert_eq!(fs_lseek(fd, 0, SEEK_SET, 11).unwrap(), 0);

            let mut out = [0u8; 64];
            assert_eq!(
                fs_read(fd, out.as_mut_ptr() as u64, write_len, 11).unwrap(),
                write_len as i64
            );
            assert_eq!(&out[..write_len], &payload[..write_len]);
            assert_eq!(fs_close(fd, 11).unwrap(), 0);
        }
    }

    #[test]
    fn test_fs_openat_at_fdcwd_aliases_open() {
        init_bridge();

        let path = b"relative/exo.conf";
        let fd = fs_openat(
            AT_FDCWD,
            path,
            open_flags::O_RDWR | open_flags::O_CREAT,
            0,
            17,
        )
        .unwrap() as u32;
        assert!(fd >= crate::fs::exofs::syscall::object_fd::FD_RESERVED);
        assert_eq!(fs_close(fd, 17).unwrap(), 0);
    }

    #[test]
    fn test_fs_openat_relative_dirfd_is_rejected() {
        init_bridge();

        let err = fs_openat(9, b"relative/exo.conf", open_flags::O_RDONLY, 0, 17).unwrap_err();
        assert_eq!(err, FsBridgeError::Invalid);
    }

    #[test]
    fn test_fs_stat_and_fstat_report_size_and_kind() {
        init_bridge();

        let dir_path = b"/var/exo";
        assert_eq!(fs_mkdir(dir_path, 0, 21).unwrap(), 0);

        let mut dir_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(dir_path, &mut dir_stat as *mut _ as u64, 21).unwrap(),
            0
        );
        assert_eq!(
            dir_stat.st_mode & STAT_MODE_MASK,
            STAT_MODE_DIR & STAT_MODE_MASK
        );

        let file_path = b"/var/exo/runtime.cfg";
        let payload = *b"exo-runtime";
        let fd =
            fs_open(file_path, open_flags::O_RDWR | open_flags::O_CREAT, 0, 21).unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 21).unwrap(),
            payload.len() as i64
        );

        let mut path_stat = LinuxStat::default();
        let mut fd_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(file_path, &mut path_stat as *mut _ as u64, 21).unwrap(),
            0
        );
        assert_eq!(fs_fstat(fd, &mut fd_stat as *mut _ as u64, 21).unwrap(), 0);
        assert_eq!(path_stat.st_size, payload.len() as i64);
        assert_eq!(fd_stat.st_size, payload.len() as i64);
        assert_eq!(
            fd_stat.st_mode & STAT_MODE_MASK,
            STAT_MODE_FILE & STAT_MODE_MASK
        );
        assert_eq!(fs_close(fd, 21).unwrap(), 0);
    }

    #[test]
    fn test_fs_mkdir_rmdir_unlink_roundtrip() {
        init_bridge();

        let dir_path = b"/srv/exo";
        assert_eq!(fs_mkdir(dir_path, 0, 29).unwrap(), 0);
        assert_eq!(fs_unlink(dir_path, 29).unwrap_err(), FsBridgeError::IsDir);
        assert_eq!(fs_rmdir(dir_path, 29).unwrap(), 0);
        let mut dir_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(dir_path, &mut dir_stat as *mut _ as u64, 29).unwrap_err(),
            FsBridgeError::NotFound
        );

        let file_path = b"/srv/exo.log";
        let fd =
            fs_open(file_path, open_flags::O_CREAT | open_flags::O_RDWR, 0, 29).unwrap() as u32;
        assert_eq!(fs_close(fd, 29).unwrap(), 0);
        assert_eq!(fs_unlink(file_path, 29).unwrap(), 0);
        let mut file_stat = LinuxStat::default();
        assert_eq!(
            fs_stat(file_path, &mut file_stat as *mut _ as u64, 29).unwrap_err(),
            FsBridgeError::NotFound
        );
    }

    #[test]
    fn test_fs_directory_lifecycle_stress() {
        init_bridge();

        for idx in 0..256u32 {
            let mut dir_path = [0u8; 40];
            let dir_prefix = b"/dirs/";
            dir_path[..dir_prefix.len()].copy_from_slice(dir_prefix);
            let dir_len = dir_prefix.len() + write_u32_hex(&mut dir_path[dir_prefix.len()..], idx);

            assert_eq!(fs_mkdir(&dir_path[..dir_len], 0, 42).unwrap(), 0);
            let mut dir_stat = LinuxStat::default();
            assert_eq!(
                fs_stat(&dir_path[..dir_len], &mut dir_stat as *mut _ as u64, 42).unwrap(),
                0
            );
            assert_eq!(
                dir_stat.st_mode & STAT_MODE_MASK,
                STAT_MODE_DIR & STAT_MODE_MASK
            );
            assert_eq!(fs_rmdir(&dir_path[..dir_len], 42).unwrap(), 0);

            let mut file_path = [0u8; 48];
            let file_prefix = b"/files/";
            file_path[..file_prefix.len()].copy_from_slice(file_prefix);
            let file_len =
                file_prefix.len() + write_u32_hex(&mut file_path[file_prefix.len()..], idx);
            let fd = fs_open(
                &file_path[..file_len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                42,
            )
            .unwrap() as u32;
            assert_eq!(fs_close(fd, 42).unwrap(), 0);
            assert_eq!(fs_unlink(&file_path[..file_len], 42).unwrap(), 0);
        }
    }

    fn parse_dirent_names(buf: &[u8]) -> Vec<Vec<u8>> {
        let mut names = Vec::new();
        let mut off = 0usize;
        while off + DIRENT64_HEADER_SIZE <= buf.len() {
            let reclen = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
            if reclen == 0 || off + reclen > buf.len() {
                break;
            }
            let name_start = off + DIRENT64_HEADER_SIZE;
            let name_end = off + reclen;
            let mut cursor = name_start;
            while cursor < name_end && buf[cursor] != 0 {
                cursor += 1;
            }
            names.push(buf[name_start..cursor].to_vec());
            off += reclen;
        }
        names
    }

    #[test]
    fn test_fs_getdents64_reports_created_children() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/etc", 0, 51).unwrap(), 0);
        let file_fd = fs_open(
            b"/etc/hosts",
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            51,
        )
        .unwrap() as u32;
        assert_eq!(fs_close(file_fd, 51).unwrap(), 0);

        let dir_fd = fs_open(b"/etc", open_flags::O_RDONLY, 0, 51).unwrap() as u32;
        let mut out = [0u8; 256];
        let n = fs_getdents64(dir_fd, out.as_mut_ptr() as u64, out.len(), 51).unwrap() as usize;
        let names = parse_dirent_names(&out[..n]);
        assert!(names.iter().any(|name| name.as_slice() == b"hosts"));
        assert_eq!(fs_close(dir_fd, 51).unwrap(), 0);
    }

    #[test]
    fn test_fs_getdents64_stress_tracks_many_children() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/stressdir", 0, 52).unwrap(), 0);
        for idx in 0..64u32 {
            let mut path = [0u8; 32];
            let prefix = b"/stressdir/f";
            path[..prefix.len()].copy_from_slice(prefix);
            let len = prefix.len() + write_u32_hex(&mut path[prefix.len()..], idx);
            let fd = fs_open(
                &path[..len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                52,
            )
            .unwrap() as u32;
            assert_eq!(fs_close(fd, 52).unwrap(), 0);
        }

        let dir_fd = fs_open(b"/stressdir", open_flags::O_RDONLY, 0, 52).unwrap() as u32;
        let mut out = [0u8; 4096];
        let n = fs_getdents64(dir_fd, out.as_mut_ptr() as u64, out.len(), 52).unwrap() as usize;
        let names = parse_dirent_names(&out[..n]);
        assert_eq!(names.len(), 64);
        assert!(names.iter().any(|name| name.as_slice() == b"f0"));
        assert!(names.iter().any(|name| name.as_slice() == b"f3f"));
        assert_eq!(fs_close(dir_fd, 52).unwrap(), 0);
    }

    #[test]
    fn test_fs_symlink_roundtrip_and_following() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/opt", 0, 61).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/opt/bin", 0, 61).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/usr", 0, 61).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/usr/bin", 0, 61).unwrap(), 0);

        let payload = *b"exo-shell";
        let target_path = b"/opt/bin/exo";
        let fd = fs_open(
            target_path,
            open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
            0,
            61,
        )
        .unwrap() as u32;
        assert_eq!(
            fs_write(fd, payload.as_ptr() as u64, payload.len(), 61).unwrap(),
            payload.len() as i64
        );
        assert_eq!(fs_close(fd, 61).unwrap(), 0);

        let target = b"../../opt/bin/exo";
        assert_eq!(fs_symlink(target, b"/usr/bin/exo", 61).unwrap(), 0);

        let mut readlink_out = [0u8; 32];
        let readlink_len = fs_readlink(
            b"/usr/bin/exo",
            readlink_out.as_mut_ptr() as u64,
            readlink_out.len(),
            61,
        )
        .unwrap() as usize;
        assert_eq!(&readlink_out[..readlink_len], target);

        let fd = fs_open(b"/usr/bin/exo", open_flags::O_RDONLY, 0, 61).unwrap() as u32;
        let mut out = [0u8; 16];
        let n = fs_read(fd, out.as_mut_ptr() as u64, out.len(), 61).unwrap() as usize;
        assert_eq!(&out[..n], &payload);
        assert_eq!(fs_close(fd, 61).unwrap(), 0);

        let mut lstat_buf = LinuxStat::default();
        let mut stat_buf = LinuxStat::default();
        assert_eq!(
            fs_lstat(b"/usr/bin/exo", &mut lstat_buf as *mut _ as u64, 61).unwrap(),
            0
        );
        assert_eq!(
            fs_stat(b"/usr/bin/exo", &mut stat_buf as *mut _ as u64, 61).unwrap(),
            0
        );
        assert_eq!(
            lstat_buf.st_mode & STAT_MODE_MASK,
            STAT_MODE_SYMLINK & STAT_MODE_MASK
        );
        assert_eq!(
            stat_buf.st_mode & STAT_MODE_MASK,
            STAT_MODE_FILE & STAT_MODE_MASK
        );
    }

    #[test]
    fn test_fs_symlink_stress_many_relative_links() {
        init_bridge();

        assert_eq!(fs_mkdir(b"/targets", 0, 62).unwrap(), 0);
        assert_eq!(fs_mkdir(b"/links", 0, 62).unwrap(), 0);

        for idx in 0..96u32 {
            let mut target_path = [0u8; 32];
            let target_prefix = b"/targets/t";
            target_path[..target_prefix.len()].copy_from_slice(target_prefix);
            let target_len =
                target_prefix.len() + write_u32_hex(&mut target_path[target_prefix.len()..], idx);

            let fd = fs_open(
                &target_path[..target_len],
                open_flags::O_CREAT | open_flags::O_RDWR | open_flags::O_TRUNC,
                0,
                62,
            )
            .unwrap() as u32;
            let payload = [idx as u8; 4];
            assert_eq!(
                fs_write(fd, payload.as_ptr() as u64, payload.len(), 62).unwrap(),
                payload.len() as i64
            );
            assert_eq!(fs_close(fd, 62).unwrap(), 0);

            let mut link_path = [0u8; 32];
            let link_prefix = b"/links/l";
            link_path[..link_prefix.len()].copy_from_slice(link_prefix);
            let link_len =
                link_prefix.len() + write_u32_hex(&mut link_path[link_prefix.len()..], idx);

            let mut target_rel = [0u8; 24];
            let rel_prefix = b"../targets/t";
            target_rel[..rel_prefix.len()].copy_from_slice(rel_prefix);
            let rel_len =
                rel_prefix.len() + write_u32_hex(&mut target_rel[rel_prefix.len()..], idx);

            assert_eq!(
                fs_symlink(&target_rel[..rel_len], &link_path[..link_len], 62).unwrap(),
                0
            );

            let fd = fs_open(&link_path[..link_len], open_flags::O_RDONLY, 0, 62).unwrap() as u32;
            let mut out = [0u8; 4];
            assert_eq!(
                fs_read(fd, out.as_mut_ptr() as u64, out.len(), 62).unwrap(),
                out.len() as i64
            );
            assert_eq!(out, [idx as u8; 4]);
            assert_eq!(fs_close(fd, 62).unwrap(), 0);
        }
    }

    fn write_u32_hex(dst: &mut [u8], value: u32) -> usize {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut shift = 28u32;
        let mut out = 0usize;
        let mut started = false;

        loop {
            let nibble = ((value >> shift) & 0xF) as usize;
            if nibble != 0 || started || shift == 0 {
                dst[out] = HEX[nibble];
                out += 1;
                started = true;
            }
            if shift == 0 {
                break;
            }
            shift -= 4;
        }

        out
    }
}
