#![no_std]

extern crate alloc;

pub mod address;
pub mod capability;
pub mod error;
pub mod errno;
pub mod fd;
pub mod pid;
pub mod signal;
pub mod syscall;
pub mod time;
pub mod uid_gid;

// Réexportations
pub use address::{PhysAddr, VirtAddr, PAGE_SIZE, HUGE_PAGE_SIZE, GIGA_PAGE_SIZE};
pub use capability::{Capability, CapabilityMetadata, CapabilityType, Rights};
pub use error::{ErrorCode, ExoError};
pub use errno::Errno;
pub use fd::{BorrowedFd, FileDescriptor};
pub use pid::Pid;
pub use signal::Signal;
pub use syscall::SyscallNumber;
pub use time::{Duration, Timestamp};

/// Type Result standard utilisant Errno
pub type Result<T> = core::result::Result<T, Errno>;

/// Type Result utilisant ExoError
pub type ExoResult<T> = core::result::Result<T, ExoError>;
