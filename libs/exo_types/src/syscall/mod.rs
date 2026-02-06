//! System call interface (Layer 3)
//!
//! System call numbers and raw assembly wrappers.

pub mod numbers;
pub mod raw;

pub use numbers::SyscallNumber;
pub use raw::{syscall0, syscall1, syscall2, syscall3, syscall4, syscall5, syscall6};
