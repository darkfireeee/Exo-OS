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


use core::sync::atomic::{AtomicBool, Ordering};

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

// ─────────────────────────────────────────────────────────────────────────────
// Stubs I/O — À REMPLACER par les vrais appels fs/ lors de l'activation
// ─────────────────────────────────────────────────────────────────────────────

/// `read(fd, buf, count)` → octets lus.
/// PONT : `crate::fs::vfs::read(fd, buf_slice)` — activé quand `pub mod fs;`
#[inline]
pub fn fs_read(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (fd, buf_ptr, count, pid);
    // A_FAIRE: crate::fs::vfs::sys_read(fd, buf_ptr, count, pid)
    Err(FsBridgeError::NotReady)
}

/// `write(fd, buf, count)` → octets écrits.
#[inline]
pub fn fs_write(fd: u32, buf_ptr: u64, count: usize, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (fd, buf_ptr, count, pid);
    // A_FAIRE: crate::fs::vfs::sys_write(fd, buf_ptr, count, pid)
    Err(FsBridgeError::NotReady)
}

/// `open(path, flags, mode)` → fd.
#[inline]
pub fn fs_open(path: &[u8], flags: u32, mode: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (path, flags, mode, pid);
    // A_FAIRE: crate::fs::vfs::sys_open(path, flags, mode, pid)
    Err(FsBridgeError::NotReady)
}

/// `close(fd)`.
#[inline]
pub fn fs_close(fd: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (fd, pid);
    // A_FAIRE: crate::fs::vfs::sys_close(fd, pid)
    Err(FsBridgeError::NotReady)
}

/// `lseek(fd, offset, whence)` → nouvelle position.
#[inline]
pub fn fs_lseek(fd: u32, offset: i64, whence: u32, pid: u32) -> Result<i64, FsBridgeError> {
    if !is_fs_ready() { return Err(FsBridgeError::NotReady); }
    let _ = (fd, offset, whence, pid);
    Err(FsBridgeError::NotReady)
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
