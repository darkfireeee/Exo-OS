//! CPU abstraction layer
//!
//! Architecture-independent CPU interface

#[cfg(target_arch = "x86_64")]
pub use crate::arch::x86_64::cpu::*;

// Add other architectures as needed
// #[cfg(target_arch = "aarch64")]
// pub use crate::arch::aarch64::cpu::*;
