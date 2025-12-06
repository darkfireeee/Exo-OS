//! Pseudo filesystems module
//!
//! Contains implementations of virtual/pseudo filesystems:
//! - devfs: Device filesystem (/dev)
//! - procfs: Process information filesystem (/proc)
//! - sysfs: System information filesystem (/sys)
//! - tmpfs: Temporary RAM-based filesystem

/// Device filesystem (/dev)
pub mod devfs;

/// Process information filesystem (/proc)
pub mod procfs;

/// System information filesystem (/sys)
pub mod sysfs;

/// Temporary RAM filesystem
pub mod tmpfs;
