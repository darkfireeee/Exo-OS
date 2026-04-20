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
use core::sync::atomic::{AtomicBool, Ordering};

use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::core::{BlobId, ExofsError};
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::syscall::validation::{copy_from_user, copy_to_user};

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
    /// Erreur I/O non spécifiée.
    Io,
}

impl FsBridgeError {
    /// Convertit en errno POSIX (valeur négative).
    pub fn to_errno(self) -> i64 {
        match self {
            FsBridgeError::NotReady   => -38, // ENOSYS
            FsBridgeError::BadFd      => -9,  // EBADF
            FsBridgeError::BadPath    => -22, // EINVAL
            FsBridgeError::NotFound   => -2,  // ENOENT
            FsBridgeError::PermDenied => -13, // EACCES
            FsBridgeError::Fault      => -14, // EFAULT
            FsBridgeError::Invalid    => -22, // EINVAL
            FsBridgeError::NoSpace    => -28, // ENOSPC
            FsBridgeError::Exists     => -17, // EEXIST
            FsBridgeError::NotDir     => -20, // ENOTDIR
            FsBridgeError::IsDir      => -21, // EISDIR
            FsBridgeError::Io         => -5,  // EIO
        }
    }
}

const SEEK_SET: u32 = 0;
const SEEK_CUR: u32 = 1;
const SEEK_END: u32 = 2;

#[inline]
fn exofs_to_bridge_error(err: ExofsError) -> FsBridgeError {
    match err {
        ExofsError::ObjectNotFound
        | ExofsError::BlobNotFound
        | ExofsError::NotFound => FsBridgeError::NotFound,
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
        ExofsError::DirectoryNotEmpty => FsBridgeError::IsDir,
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

/// `read(fd, buf, count)` → octets lus.
/// PONT : `crate::fs::vfs::read(fd, buf_slice)` — activé quand `pub mod fs;`
#[inline]
pub fn fs_read(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
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
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
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
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = mode;
    if !open_flags::validate(flags) {
        return Err(FsBridgeError::Invalid);
    }

    let blob_id = blob_id_for_path(path)?;
    let exists = BLOB_CACHE.contains(&blob_id);

    if !exists && flags & open_flags::O_CREAT == 0 {
        return Err(FsBridgeError::NotFound);
    }
    if exists && (flags & open_flags::O_CREAT != 0) && (flags & open_flags::O_EXCL != 0) {
        return Err(FsBridgeError::Exists);
    }
    if !exists {
        ensure_blob_exists(blob_id)?;
    }
    if flags & open_flags::O_TRUNC != 0 {
        if !open_flags::can_write(flags) {
            return Err(FsBridgeError::Invalid);
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
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
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
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = pid;
    let entry = OBJECT_TABLE.get(fd).map_err(exofs_to_bridge_error)?;
    let base = match whence {
        SEEK_SET => 0i64,
        SEEK_CUR => entry.cursor as i64,
        SEEK_END => snapshot_blob(&entry.blob_id).map(|data| data.len() as i64).unwrap_or(entry.size as i64),
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
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (path, stat_ptr, pid);
    Err(FsBridgeError::NotReady)
}

/// `fstat(fd, stat_ptr)`.
#[inline]
pub fn fs_fstat(fd: u32, stat_ptr: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (fd, stat_ptr, pid);
    Err(FsBridgeError::NotReady)
}

/// `openat(dirfd, path, flags, mode)`.
#[inline]
pub fn fs_openat(dirfd: i32, path: &[u8], flags: u32, mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (dirfd, path, flags, mode, pid);
    Err(FsBridgeError::NotReady)
}

/// `dup(oldfd)`.
#[inline]
pub fn fs_dup(oldfd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (oldfd, pid);
    Err(FsBridgeError::NotReady)
}

/// `dup2(oldfd, newfd)`.
#[inline]
pub fn fs_dup2(oldfd: u32, newfd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (oldfd, newfd, pid);
    Err(FsBridgeError::NotReady)
}

/// `fcntl(fd, cmd, arg)`.
#[inline]
pub fn fs_fcntl(fd: u32, cmd: u32, arg: u64, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (fd, cmd, arg, pid);
    Err(FsBridgeError::NotReady)
}

/// `mkdir(path, mode)`.
#[inline]
pub fn fs_mkdir(path: &[u8], mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (path, mode, pid);
    Err(FsBridgeError::NotReady)
}

/// `rmdir(path)`.
#[inline]
pub fn fs_rmdir(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (path, pid);
    Err(FsBridgeError::NotReady)
}

/// `unlink(path)`.
#[inline]
pub fn fs_unlink(path: &[u8], pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (path, pid);
    Err(FsBridgeError::NotReady)
}

/// `getdents64(fd, dirp, count)`.
#[inline]
pub fn fs_getdents64(fd: u32, dirp: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (fd, dirp, count, pid);
    Err(FsBridgeError::NotReady)
}

/// `readlink(path, buf, bufsize)`.
#[inline]
pub fn fs_readlink(path: &[u8], buf: u64, bufsize: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (path, buf, bufsize, pid);
    Err(FsBridgeError::NotReady)
}

/// `readlinkat(dirfd, path, buf, bufsize)`.
#[inline]
pub fn fs_readlinkat(
    dirfd: i32, path: &[u8], buf: u64, bufsize: usize, pid: u32,
) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (dirfd, path, buf, bufsize, pid);
    Err(FsBridgeError::NotReady)
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper : conversion automatique pour les syscall handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit un `Result<i64, FsBridgeError>` en code de retour syscall.
#[inline(always)]
pub fn bridge_result(r: Result<i64, FsBridgeError>) -> i64 {
    match r {
        Ok(n)  => n,
        Err(e) => e.to_errno(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_bridge() {
        unsafe { fs_bridge_init(); }
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
            let len = 8;
            path[..len].copy_from_slice(b"/stress/");
            path[len..len + 4].copy_from_slice(&idx.to_le_bytes());

            let write_len = (idx as usize % 48) + 1;
            let mut payload = [0u8; 64];
            for (off, byte) in payload[..write_len].iter_mut().enumerate() {
                *byte = idx.wrapping_add(off as u32) as u8;
            }

            let fd = fs_open(
                &path[..len + 4],
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
}
