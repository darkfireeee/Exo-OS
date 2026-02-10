//! Filesystem utilities
//!
//! Common utilities used across the filesystem subsystem:
//! - Bitmap operations for allocation tracking
//! - CRC/checksum calculations
//! - Endianness conversions
//! - Lock-free primitives
//! - Time utilities

pub mod bitmap;
pub mod crc;
pub mod endian;
pub mod locks;
pub mod time;
pub mod math;

// Re-exports
pub use bitmap::Bitmap;
pub use locks::{SpinLock, SeqLock, AtomicCounter, AtomicRefCount};
pub use math::{exp_approx, sqrt_approx, floor_approx, powi_approx, log2_approx_f32, log2_approx_f64};
