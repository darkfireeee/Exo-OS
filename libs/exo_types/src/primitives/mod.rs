//! Primitive system types (Layer 0)
//!
//! Low-level types with no dependencies on other modules.
//! All types are zero-cost abstractions with validation.

// Re-export address types from root module
pub use crate::address::{PhysAddr, VirtAddr, PAGE_SIZE, HUGE_PAGE_SIZE, GIGA_PAGE_SIZE};

// Local primitive modules
pub mod pid;
pub mod fd;
pub mod uid_gid;

// Re-export primitive types
pub use pid::{Pid, KERNEL_PID, INIT_PID};
pub use fd::{Fd, STDIN, STDOUT, STDERR};
pub use uid_gid::{Uid, Gid, ROOT_UID, ROOT_GID, NOBODY_UID, NOBODY_GID};
