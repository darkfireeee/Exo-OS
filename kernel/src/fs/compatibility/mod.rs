// kernel/src/fs/compatibility/mod.rs
//
// Couche de compatibilité POSIX 2024 + Linux étendu.

pub mod posix;
pub mod linux_compat;

pub use posix::{
    posix_open, posix_close, posix_read, posix_write, posix_lseek,
    posix_stat, posix_fstat, posix_lstat,
    posix_chmod, posix_chown,
    posix_mkdir, posix_rmdir, posix_unlink, posix_rename,
    posix_fsync, POSIX_STATS,
};

pub use linux_compat::{
    linux_statx, linux_openat, linux_copy_file_range,
    linux_preadv2, linux_pwritev2,
    linux_fallocate, linux_ftruncate,
    StatxMask, Statx, IoVec, IoVecConst, LINUX_STATS,
};
