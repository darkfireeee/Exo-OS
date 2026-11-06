//! # syscall/fs_bridge — Exo-OS VFS / POSIX bridge
//!
//! This module segregates FS bridge features into specialized submodules:
//! - `types`     : FsBridgeError errors, POSIX ABI structures (Stat, Dirent...) and conversions
//! - `cwd`       : Per-process current working directory (CWD), umask and rlimit bounds
//! - `fd`        : File descriptor resolution and management (open/close/dup/fcntl/flock)
//! - `special`   : TTY devices, IPC pseudo-files (pipe, socketpair, eventfd, splice)
//! - `file_ops`  : Direct blob access primitives and I/O operations (read, write, lseek...)
//! - `path_ops`  : Tree path resolution, indexing, symlinks and metadata
//! - `poll`      : Synchronous and asynchronous I/O multiplexing (poll, select, epoll, inotify)
//! - `tests`     : Unit and integration test suite

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

/// Indicates whether the fs/ subsystem is initialized and ready.
/// Set to `true` by `fs_bridge_init()` during boot.
static FS_READY: AtomicBool = AtomicBool::new(false);

/// Initializes the fs bridge.
/// To be called from the boot sequence after `fs::init()`.
///
/// # Safety
/// Must be called exactly once from the BSP.
pub unsafe fn fs_bridge_init() {
    FS_READY.store(true, Ordering::Release);
}

/// Returns `true` if the fs/ subsystem is ready.
#[inline(always)]
pub fn is_fs_ready() -> bool {
    FS_READY.load(Ordering::Acquire)
}
