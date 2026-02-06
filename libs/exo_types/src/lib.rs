//! Exo-OS Type Library
//!
//! Production-grade type definitions for Exo-OS kernel and userspace.
//!
//! # Architecture
//!
//! This library follows a layered architecture:
//!
//! - **Layer 0 (Primitives)**: Fundamental types with no dependencies
//!   - Address types (PhysAddr, VirtAddr)
//!   - Process/thread IDs (Pid, Tid)
//!   - File descriptors (Fd)
//!   - User/Group IDs (Uid, Gid)
//!
//! - **Layer 1 (Error & Time)**: Types with minimal dependencies
//!   - Error codes (Errno)
//!   - Time types (Timestamp, Duration)
//!
//! - **Layer 2 (IPC & Security)**: Communication and security primitives
//!   - Signals (Signal, SignalSet)
//!   - Capabilities (Capability, Rights)
//!
//! - **Layer 3 (Syscalls)**: System call interface
//!   - Syscall numbers
//!   - Assembly wrappers
//!
//! # Features
//!
//! - `std`: Enable standard library support (default: disabled)
//!
//! # Examples
//!
//! ```rust
//! use exo_types::prelude::*;
//!
//! // Create a physical address
//! let paddr = PhysAddr::new(0x1000);
//! assert!(paddr.is_page_aligned());
//!
//! // Create a process ID
//! let pid = Pid::new(123);
//! assert!(!pid.is_kernel());
//!
//! // Create a capability
//! let cap = Capability::new(1, CapabilityType::File, Rights::READ);
//! assert!(cap.has_rights(Rights::READ));
//! ```

#![no_std]
#![cfg_attr(not(test), deny(warnings))]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

// Allow std in tests
#[cfg(all(test, feature = "std"))]
extern crate std;

// Re-export core for use in macros
#[doc(hidden)]
pub use core;

// ===== Core Modules (Layer 0-3) =====

/// Physical and virtual address types
pub mod address;

/// Error numbers (errno)
pub mod errno;

/// Capability-based security
pub mod capability;

/// Primitive types (Layer 0)
pub mod primitives;

/// Time types (Layer 1)
pub mod time;

/// IPC types (Layer 2)
pub mod ipc;

/// System call interface (Layer 3)
pub mod syscall;

// ===== Prelude =====

/// Prelude module with commonly used types
pub mod prelude;

// ===== Top-level Re-exports =====

// Primitives
pub use address::{PhysAddr, VirtAddr, PAGE_SIZE, HUGE_PAGE_SIZE, GIGA_PAGE_SIZE};
pub use primitives::{
    Pid, Fd, Uid, Gid,
    KERNEL_PID, INIT_PID,
    STDIN, STDOUT, STDERR,
    ROOT_UID, ROOT_GID, NOBODY_UID, NOBODY_GID,
};

// Errors
pub use errno::Errno;

// Time
pub use time::{Timestamp, TimestampKind, Duration};

// IPC
pub use ipc::{Signal, SignalSet, SignalAction, SignalHandler};

// Capability
pub use capability::{
    Capability, CapabilityType, CapabilityMetadata, Rights, MetadataFlags,
};

// Syscalls
pub use syscall::{SyscallNumber, syscall0, syscall1, syscall2, syscall3, syscall4, syscall5, syscall6};
