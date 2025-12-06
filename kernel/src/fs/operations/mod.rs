//! Core filesystem operations module
//!
//! Contains fundamental filesystem operations:
//! - buffer: Advanced I/O buffering with read-ahead and write-back
//! - locks: POSIX record locks and BSD flock
//! - fdtable: Revolutionary lock-free file descriptor table
//! - cache: Path component and dentry cache

/// File buffering layer
pub mod buffer;

/// File locking (POSIX + BSD)
pub mod locks;

/// File descriptor table
pub mod fdtable;

/// Path and dentry cache
pub mod cache;
