// kernel/src/fs/core/mod.rs
//
// ─────────────────────────────────────────────────────────────────────────────
// Module fs/core — Types fondamentaux, VFS, Inodes, Dentries, Descripteurs
// ─────────────────────────────────────────────────────────────────────────────

pub mod types;
pub mod vfs;
pub mod inode;
pub mod dentry;
pub mod descriptor;
/// Superblock générique VFS (distinct des superblocks on-disk de chaque FS).
pub mod superblock;

// Re-exports fondamentaux
pub use types::{
    FileMode, FileType, InodeNumber, DevId, Stat, Dirent64, DirEntryType,
    FsError, FsResult, FsStats, OpenFlags, SeekWhence, Timespec64, MountFlags,
    Uid, Gid, InodeFlags, FsAtomics, FsGeneration, FS_STATS,
    NAME_MAX, PATH_MAX, OPEN_MAX, FS_BLOCK_SIZE, ROOT_INO, INVALID_INO, MAXSYMLINKS,
};
pub use vfs::{
    FsType, Superblock, InodeOps, FileOps, FileHandle,
    MmapFlags, PollEvents, RenameFlags, InodeAttr,
    MountEntry, MountTable, MOUNT_TABLE,
    FsTypeRegistry, FS_TYPE_REGISTRY,
    LookupContext, LookupResult, path_lookup,
    vfs_mount, vfs_umount, vfs_init,
};
pub use inode::{
    Inode, InodeRef, InodeState,
    new_inode_ref, INODE_CACHE, InodeCacheSimple,
};
pub use dentry::{
    Dentry, DentryRef, DentryName, DentryState,
    DENTRY_CACHE, DentryCache,
};
pub use descriptor::{
    Fd, FdEntry, FdTable, CurrentDir, FdStats, FD_STATS,
};
pub use superblock::{
    VfsSuperblock, VfsSuperblockRef, FsOps, FsStatInfo, MountMode,
    SUPERBLOCK_TABLE, SbTableStats, SB_TABLE_STATS,
};
