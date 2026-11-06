//! # syscall/fs_bridge/path_ops/mod.rs — Path resolution, tree traversal, symlinks and metadata
//!
//! This module segregates filesystem traversal and manipulation logic into submodules:
//! - `resolution` : Path normalization, Blake3 hashing, traversal and directory indexing.
//! - `operations` : POSIX VFS primitives (open, stat, lstat, fstat, mkdir, rmdir, unlink, rename, link).
//! - `symlinks`   : POSIX symbolic link handling and resolution (symlink, readlink).
//! - `metadata`   : Directory reading (getdents64), permissions, attributes, statx, synchronization.

pub mod metadata;
pub mod operations;
pub mod resolution;
pub mod symlinks;

pub use metadata::*;
pub use operations::*;
pub use resolution::*;
pub use symlinks::*;
