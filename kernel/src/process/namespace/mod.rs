// kernel/src/process/namespace/mod.rs
//
// Espaces de noms Linux (namespaces) — Couche 1.5 Exo-OS.

pub mod mount_ns;
pub mod net_ns;
pub mod pid_ns;
pub mod user_ns;
pub mod uts_ns;

pub use mount_ns::{MountNamespace, ROOT_MOUNT_NS};
pub use net_ns::{NetNamespace, ROOT_NET_NS};
pub use pid_ns::{PidNamespace, ROOT_PID_NS};
pub use user_ns::{UserNamespace, ROOT_USER_NS};
pub use uts_ns::{UtsNamespace, ROOT_UTS_NS};

/// Ensemble des namespaces actifs pour un processus.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct NsSet {
    pub pid_ns: u32, // index dans PID_NS_TABLE
    pub mnt_ns: u32,
    pub net_ns: u32,
    pub uts_ns: u32,
    pub user_ns: u32,
}

impl NsSet {
    /// NsSet pour le namespace raçine (init).
    pub const ROOT: Self = Self {
        pid_ns: 0,
        mnt_ns: 0,
        net_ns: 0,
        uts_ns: 0,
        user_ns: 0,
    };
}
