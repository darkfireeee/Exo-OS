#![no_std]

extern crate alloc;

pub mod address;
pub mod capability;
pub mod error;
pub mod pid;
pub mod fd;
pub mod errno;
pub mod time;
pub mod syscall;
// pub mod signal; // TODO: Create signal module
pub mod uid_gid;

// Réexportations
pub use address::{PhysAddr, VirtAddr};
pub use capability::{Capability, CapabilityMetadata, CapabilityType, Rights};
pub use error::{ExoError, ErrorCode, Result};
pub use pid::Pid;
pub use fd::{FileDescriptor, BorrowedFd};
pub use errno::Errno;
pub use time::{Timestamp, Duration};
pub use syscall::SyscallNumber;

// Initialisation globale
pub fn init() {
    log::trace!("exo_types initialized");
}
