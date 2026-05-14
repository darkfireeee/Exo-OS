#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FsPortKind {
    BlockFilesystem,
    UserlandFilesystem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FsPort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: FsPortKind,
    pub exo_boundary: &'static str,
}

pub const FS_PORTS: &[FsPort] = &[
    FsPort {
        name: "fatfs",
        vendor_tree: "rust-fatfs-upstream",
        kind: FsPortKind::BlockFilesystem,
        exo_boundary: "drivers/fs FAT32 compatibility",
    },
    FsPort {
        name: "ext4-rs",
        vendor_tree: "ext4-rs-upstream",
        kind: FsPortKind::BlockFilesystem,
        exo_boundary: "drivers/fs ext4 compatibility",
    },
    FsPort {
        name: "redoxfs",
        vendor_tree: "redoxfs-upstream",
        kind: FsPortKind::BlockFilesystem,
        exo_boundary: "vfs_server userland mount backend",
    },
    FsPort {
        name: "libfuse",
        vendor_tree: "libfuse-upstream",
        kind: FsPortKind::UserlandFilesystem,
        exo_boundary: "Ring3 FUSE bridge",
    },
];

pub fn fs_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f46_5300_u64;
    for i in 0..iterations.max(1) {
        let port = FS_PORTS[i as usize % FS_PORTS.len()];
        acc = acc.rotate_left(9) ^ port.name.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}
