#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FsPortKind {
    BlockFilesystem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FsVerdict {
    Native,
    CompatibilityOnly,
    Rejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FsPort {
    pub name: &'static str,
    pub vendor_tree: &'static str,
    pub kind: FsPortKind,
    pub exo_boundary: &'static str,
    pub verdict: FsVerdict,
    pub phoenix_policy: &'static str,
}

pub const FS_PORTS: &[FsPort] = &[
    FsPort {
        name: "fatfs",
        vendor_tree: "rust-fatfs-upstream",
        kind: FsPortKind::BlockFilesystem,
        exo_boundary: "drivers/fs FAT32 compatibility",
        verdict: FsVerdict::Native,
        phoenix_policy: "invalidate-block-cache-before-switch",
    },
    FsPort {
        name: "ext4-rs",
        vendor_tree: "ext4-rs-upstream",
        kind: FsPortKind::BlockFilesystem,
        exo_boundary: "drivers/fs ext4 compatibility",
        verdict: FsVerdict::CompatibilityOnly,
        phoenix_policy: "bar-writes-during-exofs-epoch-switch",
    },
    FsPort {
        name: "redoxfs",
        vendor_tree: "redoxfs-upstream",
        kind: FsPortKind::BlockFilesystem,
        exo_boundary: "vfs_server userland mount backend",
        verdict: FsVerdict::Native,
        phoenix_policy: "flush-journal-and-cache-before-switch",
    },
];

pub fn fs_port_allowed(name: &str) -> bool {
    FS_PORTS
        .iter()
        .find(|port| port.name == name)
        .map(|port| port.verdict != FsVerdict::Rejected)
        .unwrap_or(false)
}

pub fn fs_stress_signature(iterations: u32) -> u64 {
    let mut acc = 0x4558_4f46_5300_u64;
    for i in 0..iterations.max(1) {
        let port = FS_PORTS[i as usize % FS_PORTS.len()];
        acc = acc.rotate_left(9) ^ port.name.as_bytes()[0] as u64 ^ i as u64;
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libfuse_is_removed() {
        assert!(!fs_port_allowed("libfuse"));
        assert!(FS_PORTS.iter().all(|port| port.name != "libfuse"));
    }
}
