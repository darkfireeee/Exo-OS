// kernel/src/fs/pseudo/mod.rs
//
// Pseudo FS — procfs, sysfs, devfs, tmpfs.

pub mod procfs;
pub mod sysfs;
pub mod devfs;
pub mod tmpfs;

pub use procfs::{ProcStats, PROC_STATS, ProcRootOps, ProcFileOps};
pub use sysfs::{SysfsNode, SysfsTree, SysfsAttrOps, SysfsStats, SYSFS_STATS};
pub use devfs::{DevSpecial, DevFileOps, DevfsEntry, DevfsRegistry, DevStats,
               DEVFS_REGISTRY, DEV_STATS, devfs_init};
pub use tmpfs::{TmpfsData, TmpfsDir, TmpfsNode, TmpfsFileOps, TmpfsStats,
               TMPFS_STATE, TMPFS_STATS, tmpfs_init};
