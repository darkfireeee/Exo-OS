//! # kernel/src/fs/elf_loader_impl/mod.rs
//!
//! Modular implementation of `ElfLoader` & `FileFaultProvider` backed by ExoFS:
//! - `loader`   : `ExoFsElfLoader` (`ElfLoader` + `FileFaultProvider`) & `EXO_ELF_LOADER`
//! - `trace`    : Tracing and port 0xE9 diagnostic primitives
//! - `registry` : ELF blob registry and memory segment metadata
//! - `parser`   : ELF headers and Program Headers decoding and validation
//! - `image`    : Segment and VMA installation in the address space
//! - `stack`    : Initial SysV user stack construction and argument passing
//! - `cache`    : Demand-paging dedicated ELF blob cache avoiding SLUB pressure

pub mod cache;
pub mod image;
pub mod loader;
pub mod parser;
pub mod registry;
pub mod stack;
pub mod trace;
#[cfg(test)]
mod tests;

pub use loader::{ExoFsElfLoader, EXO_ELF_LOADER};
