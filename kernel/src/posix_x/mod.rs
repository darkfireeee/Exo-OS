//! POSIX-X Compatibility Layer
//!
//! Provides POSIX compatibility on top of Exo-OS microkernel

pub mod core;
pub mod elf;
pub mod kernel_interface;
pub mod optimization;
pub mod signals;
// ⏸️ Phase 1b: pub mod syscalls;
pub mod tools;
pub mod translation;
pub mod vfs_posix;         // ✅ Phase 1: VFS POSIX adapter

// Documentation files (not a module)
// pub mod doc;
// pub mod vfs;  // TODO: Not yet implemented
pub use vfs_posix::{file_ops, VfsHandle};  // ✅ Phase 1
