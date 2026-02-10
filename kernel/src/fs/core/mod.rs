//! Core VFS Module - Hot Path Components
//!
//! This module contains the performance-critical VFS components:
//! - types: Fundamental types (InodeType, InodePermissions, Timestamp, etc.)
//! - vfs: Main VFS interface
//! - inode: Inode management
//! - dentry: Directory entry cache
//! - descriptor: File descriptors

pub mod types;
pub mod vfs;
pub mod inode;
pub mod dentry;
pub mod descriptor;

// Re-export fundamental types for convenience
pub use types::*;
pub use descriptor::FileDescriptorTable;
