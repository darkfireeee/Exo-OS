//! IPC (Inter-Process Communication) types (Layer 2)
//!
//! Types for process communication and signaling.

pub mod signal;

pub use signal::{Signal, SignalSet, SignalAction, SignalHandler};
