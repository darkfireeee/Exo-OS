//! mod.rs — POSIX bridge for ExoFS (wiring facade).

mod bridge;
pub mod fcntl_lock;
pub mod inode_emulation;
pub mod mmap;
pub mod vfs_compat;

pub use bridge::{
    atomic_saturating_dec, atomic_saturating_inc, copy_name_to_buf, flags_require_write,
    inode_flags_to_mode_type, mode_to_inode_flags, names_equal, posix_bridge_check,
    posix_bridge_diag, posix_bridge_init, posix_bridge_process_exit, posix_bridge_shutdown,
    posix_bridge_stats, posix_bridge_version, strnlen, total_lock_count, PosixBridgeConfig,
    PosixBridgeDiag, PosixBridgeStats, POSIX_BRIDGE_MAGIC, POSIX_BRIDGE_MAJOR, POSIX_BRIDGE_MINOR,
    POSIX_BRIDGE_PATCH,
};
pub use fcntl_lock::{
    ByteRangeLock, FcntlCmd, LockInfo, LockKind, FCNTL_LOCK_TABLE, MAX_LOCKS_PER_OBJECT,
    MAX_OBJECTS_LOCKED,
};
pub use inode_emulation::{
    decode_inode_entry, encode_inode_entry, inode_flags, InodeEntry, ObjectIno, INODE_EMULATION,
    INO_MAX_CACHE, INO_RESERVED, INO_ROOT,
};
pub use mmap::{
    align_up, map_flags, map_prot, pages_for, ranges_overlap, validate_page_aligned, MappingState,
    MmapEntry, MMAP_MAX_MAPPINGS, MMAP_PAGE_SIZE, MMAP_TABLE,
};
pub use vfs_compat::{
    file_mode, open_flags, register_exofs_vfs_ops, root_inode, vfs_close, vfs_close_all_pid,
    vfs_create, vfs_getattr, vfs_is_registered, vfs_lookup, vfs_mkdir, vfs_open, vfs_open_count,
    vfs_read, vfs_readdir, vfs_rename, vfs_rmdir, vfs_symlink, vfs_truncate, vfs_unlink, vfs_write,
    VfsDirent, VfsFd, VfsInode, VFS_NAME_MAX, VFS_ROOT_INO,
};
