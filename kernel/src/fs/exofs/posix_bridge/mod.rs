//! posix_bridge/ — Pont VFS Ring 0 ExoFS (no_std).
//! Contient UNIQUEMENT les mécanismes kernel qui touchent la page table ou le VFS.

pub mod inode_emulation;
pub mod vfs_compat;
pub mod mmap;
pub mod fcntl_lock;

pub use inode_emulation::{InodeEmulation, ObjectIno};
pub use vfs_compat::register_exofs_vfs_ops;
