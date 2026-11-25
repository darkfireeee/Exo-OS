//! ExoFS — Native Epoch-Journaled File System (Ring 0, `no_std`, ISA-independent core).
//!
//! Hardware adapters (VirtIO-Blk, ATA PIO, Multiboot2 ramdisk, x86 clock/RNG/trace)
//! live outside `exofs/` in `crate::fs::exofs_hal` behind `platform::ExofsPlatform`.

pub mod audit;
pub mod cache;
pub mod compress;
pub mod core;
pub mod crypto;
pub mod dedup;
pub mod epoch;
pub mod export;
pub mod gc;
pub mod io;
mod lifecycle;
pub mod numa;
pub mod objects;
pub mod observability;
pub mod path;
pub mod platform;
pub mod posix_bridge;
pub mod quota;
pub mod recovery;
pub mod relation;
pub mod snapshot;
pub mod storage;
pub mod syscall;

/// Unit tests, integration, fuzz (spec 2.0)
#[cfg(test)]
pub mod test_support;
#[cfg(test)]
pub mod tests;

pub use lifecycle::{exofs_init, exofs_register_fs, exofs_shutdown, unlock_encrypted_volume};
