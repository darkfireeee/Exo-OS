//! Sysfs ExoFS compatibility nodes.

pub const SYSFS_EXOFS_ROOT: &[u8] = b"/sys/fs/exofs";
pub const SYSFS_STATS: &[u8] = b"stats";
pub const SYSFS_TUNABLES: &[u8] = b"tunables";
pub const SYSFS_HEALTH: &[u8] = b"health";

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthState {
    Ok = 0,
    Degraded = 1,
    ReadOnly = 2,
    Error = 3,
}

impl HealthState {
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            HealthState::Ok => b"ok",
            HealthState::Degraded => b"degraded",
            HealthState::ReadOnly => b"readonly",
            HealthState::Error => b"error",
        }
    }
}
