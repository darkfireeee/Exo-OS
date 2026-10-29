//! System abstraction layer (`std::sys`).
//!
//! Dispatches to `exo_os` on `target_os = "exo"` (and as the primary PAL for Exo-OS).

pub mod exo_os;
pub use exo_os::*;
