//! # syscall/handlers/mod.rs — Thin wrappers syscall
//!
//! Ce module regroupe les wrappers POSIX et ExoFS.
//! RÈGLE SYS-03 : these files contiennent UNIQUEMENT des thin wrappers.
//! Toute logique métier est déléguée aux modules internes (fd::, process::, etc.).

pub mod fd;
pub mod process;
pub mod signal;
pub mod memory;
pub mod time;
pub mod misc;
pub mod fs_posix;
