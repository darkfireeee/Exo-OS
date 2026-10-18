//! # syscall/fs_bridge — Pont VFS / POSIX d'Exo-OS
//!
//! Ce module ségrègue les fonctionnalités du pont FS en sous-modules spécialisés :
//! - `types`     : Erreurs FsBridgeError, structures ABI POSIX (Stat, Dirent...) et conversions
//! - `cwd`       : Répertoire de travail courant par processus (CWD), umask et limites rlimit
//! - `fd`        : Résolution et gestion des descripteurs de fichiers (open/close/dup/fcntl/flock)
//! - `special`   : Périphériques TTY, pseudo-fichiers IPC (pipe, socketpair, eventfd, splice)
//! - `file_ops`  : Primitives d'accès direct aux blobs et opérations d'I/O (read, write, lseek...)
//! - `path_ops`  : Résolution de chemins arborescents, indexation, symlinks et métadonnées
//! - `poll`      : Multiplexage I/O synchrone et asynchrone (poll, select, epoll, inotify)
//! - `tests`     : Suite de tests unitaires et d'intégration

use core::sync::atomic::{AtomicBool, Ordering};

pub mod types;
pub mod cwd;
pub mod fd;
pub mod special;
pub mod file_ops;
pub mod path_ops;
pub mod poll;

#[cfg(test)]
pub mod tests;

pub use types::*;
pub use cwd::*;
pub use fd::*;
pub use special::*;
pub use file_ops::*;
pub use path_ops::*;
pub use poll::*;

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
