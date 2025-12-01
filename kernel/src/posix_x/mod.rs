//! POSIX-X: POSIX compatibility layer for Exo-OS
//!
//! Provides musl libc syscall support

pub mod core;
pub mod elf;
pub mod signals;
pub mod syscalls;
// pub mod vfs;  // TODO: Not yet implemented
pub mod kernel_interface;
pub mod vfs_posix;

// Re-exports
pub use vfs_posix::{file_ops, VfsHandle};
