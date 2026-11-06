//! # syscall/handlers/mod.rs — Syscall thin wrappers
//!
//! This module groups POSIX and ExoFS wrappers.
//! RULE SYS-03: these files contain ONLY thin wrappers.
//! All business logic is delegated to internal modules (fd::, process::, etc.).

pub mod fd;
pub mod fs_posix;
pub mod memory;
pub mod misc;
pub mod process;
pub mod signal;
pub mod time;
