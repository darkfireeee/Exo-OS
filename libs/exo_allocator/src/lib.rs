//! Custom memory allocators for Exo-OS
//!
//! This crate provides specialized allocators optimized for different use cases:
//! - **Slab**: Fixed-size object pools
//! - **Bump**: Arena/scratch allocations
//! - **Mimalloc**: High-performance general-purpose allocator
//!
//! # Example
//!
//! ```no_run
//! use exo_allocator::Mimalloc;
//!
//! #[global_allocator]
//! static GLOBAL: Mimalloc = Mimalloc;
//! ```

#![no_std]
#![forbid(unsafe_op_in_unsafe_fn)]

pub mod bump;
pub mod mimalloc;
pub mod oom;
pub mod slab;
pub mod telemetry;

pub use bump::BumpAllocator;
pub use mimalloc::Mimalloc;
pub use slab::SlabAllocator;

/// Allocator error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocError {
    /// Out of memory
    OutOfMemory,
    /// Invalid size (too large or zero)
    InvalidSize,
    /// Invalid alignment
    InvalidAlignment,
    /// Allocator capacity exceeded
    CapacityExceeded,
    /// Invalid allocator state (not initialized)
    InvalidState,
    /// Integer overflow in calculation
    Overflow,
}

impl core::fmt::Display for AllocError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AllocError::OutOfMemory => write!(f, "Out of memory"),
            AllocError::InvalidSize => write!(f, "Invalid allocation size"),
            AllocError::InvalidAlignment => write!(f, "Invalid alignment"),
            AllocError::CapacityExceeded => write!(f, "Allocator capacity exceeded"),
            AllocError::InvalidState => write!(f, "Invalid allocator state"),
            AllocError::Overflow => write!(f, "Integer overflow"),
        }
    }
}

pub type Result<T> = core::result::Result<T, AllocError>;
