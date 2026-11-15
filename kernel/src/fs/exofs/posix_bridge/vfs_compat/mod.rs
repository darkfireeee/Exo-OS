//! # kernel::fs::exofs::posix_bridge::vfs_compat — VFS compatibility layer
//!
//! Segregated modular organization:
//! - `types`     : POSIX constants, VfsInode, VfsDirent, VfsFd structures
//! - `fd_table`  : Open file descriptor table and PID isolation
//! - `directory` : Directory registry and in-memory hierarchy
//! - `persist`   : Namespace persistence and serialization onto ExoFS disk
//! - `ops`       : POSIX surface (lookup, create, open, read, write, getattr, readdir, ...)
//! - `helpers`   : Internal helpers for inode conversion, hashing, and name validation
//! - `tests`     : Conformance unit tests

pub mod types;
pub mod fd_table;
pub mod directory;
pub mod persist;
pub mod ops;
pub mod helpers;
#[cfg(test)]
pub mod tests;

pub use types::*;
pub use fd_table::*;
pub use directory::*;
pub use persist::*;
pub use ops::*;
pub use helpers::*;
