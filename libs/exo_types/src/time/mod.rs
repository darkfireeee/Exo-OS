//! Time-related types (Layer 1)
//!
//! Timestamp and duration types for timekeeping.

pub mod timestamp;
pub mod duration;

pub use timestamp::{Timestamp, TimestampKind};
pub use duration::Duration;
