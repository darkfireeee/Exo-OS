//! Minimal procfs formatting contract for POSIX tools.

pub const PROC_MOUNTS_PATH: &[u8] = b"/proc/mounts";
pub const PROC_SELF_MOUNTINFO_PATH: &[u8] = b"/proc/self/mountinfo";
pub const DEFAULT_MOUNT_ID: u32 = 25;
pub const DEFAULT_PARENT_ID: u32 = 1;
pub const DEFAULT_MAJOR_MINOR: &[u8] = b"259:1";

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MountInfoView {
    pub mount_id: u32,
    pub parent_id: u32,
    pub major: u32,
    pub minor: u32,
    pub root: &'static [u8],
    pub mount_point: &'static [u8],
    pub fs_type: &'static [u8],
    pub source: &'static [u8],
}

pub const ROOT_EXOFS_MOUNT: MountInfoView = MountInfoView {
    mount_id: DEFAULT_MOUNT_ID,
    parent_id: DEFAULT_PARENT_ID,
    major: 259,
    minor: 1,
    root: b"/",
    mount_point: b"/",
    fs_type: b"exofs",
    source: b"/dev/nvme0n1p1",
};
