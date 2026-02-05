// libs/exo_ipc/src/util/mod.rs
//! Utilitaires pour optimisations

pub mod atomic;
pub mod cache;
pub mod checksum;

// Réexportations
pub use atomic::{AtomicFlag, AtomicRefCount, AtomicStats, Backoff, SequenceCounter, StatsSnapshot};
pub use cache::{CachePadded, Padding, CACHE_LINE_SIZE};
pub use checksum::{adler32, checksum64, crc32c, has_hardware_crc32c};
