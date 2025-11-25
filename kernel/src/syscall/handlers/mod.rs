//! System Call Handlers
//!
//! Organized by category:
//! - io: File I/O operations
//! - ipc: Inter-process communication
//! - memory: Memory management
//! - process: Process/thread management
//! - security: Capability-based security
//! - time: Time and timers

pub mod io;
pub mod ipc;
pub mod memory;
pub mod process;
pub mod security;
pub mod time;

// Re-export commonly used types
pub use io::{Fd, FileFlags, FileStat};
pub use ipc::IpcHandle;
pub use memory::{ProtFlags, MapFlags};
pub use process::{Pid, Signal, ProcessStatus};
pub use security::{CapId, CapabilityType, Capability};
pub use time::{ClockId, TimeSpec, TimerId};
