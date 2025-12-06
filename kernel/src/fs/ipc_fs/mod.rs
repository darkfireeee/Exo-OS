//! IPC filesystems module
//!
//! Contains implementations of IPC-related filesystems:
//! - pipefs: Anonymous and named pipes (FIFOs)
//! - socketfs: Unix domain sockets
//! - symlinkfs: Symbolic links with O(1) cache

/// Pipe filesystem (anonymous & named pipes)
pub mod pipefs;

/// Unix domain socket filesystem
pub mod socketfs;

/// Symbolic link filesystem with cache
pub mod symlinkfs;
