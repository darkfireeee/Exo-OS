//! Compatibility namespace defaults for `vfs_server`.

pub mod errno;
pub mod procfs;
pub mod statfs_ext;
pub mod sysfs;
pub mod write_stream;

pub const FS_EXOFS: u8 = 1;
pub const FS_PROCFS: u8 = 2;
pub const FS_SYSFS: u8 = 3;
pub const FS_DEVFS: u8 = 4;

pub const VFS_NAMESPACE_MAGIC: u32 = 0x5654_4654; // VFS namespace/protocol layer.
pub const EXOFS_SUPERBLOCK_MAGIC: u32 = 0x4558_4F46; // ExoFS on-disk identity.

#[derive(Copy, Clone)]
pub struct PseudoMountSpec {
    pub fs_type: u8,
    pub path: &'static [u8],
    pub root_blob: u64,
}

pub const DEFAULT_PSEUDO_MOUNTS: [PseudoMountSpec; 3] = [
    PseudoMountSpec {
        fs_type: FS_PROCFS,
        path: b"/proc",
        root_blob: 0,
    },
    PseudoMountSpec {
        fs_type: FS_SYSFS,
        path: b"/sys",
        root_blob: 0,
    },
    PseudoMountSpec {
        fs_type: FS_DEVFS,
        path: b"/dev",
        root_blob: 0,
    },
];

pub const fn magic_values_are_layered() -> bool {
    VFS_NAMESPACE_MAGIC != EXOFS_SUPERBLOCK_MAGIC
}
