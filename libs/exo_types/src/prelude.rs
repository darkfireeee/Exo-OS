//! Prelude module
//!
//! Common imports for easy use of exo_types.
//!
//! # Example
//!
//! ```rust
//! use exo_types::prelude::*;
//!
//! // All common types are now available
//! let addr = PhysAddr::new(0x1000);
//! let pid = Pid::new(123);
//! let fd = Fd::new(3);
//! ```

// Primitives
pub use crate::address::{PhysAddr, VirtAddr, PAGE_SIZE, HUGE_PAGE_SIZE, GIGA_PAGE_SIZE};
pub use crate::primitives::{
    Pid, Fd, Uid, Gid,
    KERNEL_PID, INIT_PID,
    STDIN, STDOUT, STDERR,
    ROOT_UID, ROOT_GID, NOBODY_UID, NOBODY_GID,
};

// Errors
pub use crate::errno::Errno;

// Time
pub use crate::time::{Timestamp, TimestampKind, Duration};

// IPC
pub use crate::ipc::{Signal, SignalSet, SignalAction, SignalHandler};

// Capability
pub use crate::capability::{
    Capability, CapabilityType, CapabilityMetadata,
    Rights, MetadataFlags, hash_path,
};

// Syscalls
pub use crate::syscall::{
    SyscallNumber,
    syscall0, syscall1, syscall2, syscall3, syscall4, syscall5, syscall6,
};
