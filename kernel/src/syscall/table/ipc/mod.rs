//! # syscall/table/ipc/mod.rs — Native IPC, Capabilities, ExoCordon, ExoShield, and system service handlers
//!
//! This module organizes cleanly and modularly all system call handlers
//! related to inter-process communication and system security:
//! - `endpoints`     : Registration, discovery, and lifecycle of IPC mailboxes.
//! - `channel`       : Sending (send), receiving (recv), synchronous calls (call), and ExoCordon policy.
//! - `remote_memory` : Inter-process memory operations (copy_from/to_pid, mmap/munmap/mprotect_pid) reserved for root servers.
//! - `security_ops`  : Capability management (create/revoke/check), Pledge sandbox, and ExoShield monitoring.
//! - `system_ops`    : Kernel log ring buffer, PCB process snapshot, performance metrics, and Phoenix transitions.

pub mod channel;
pub mod endpoints;
pub mod remote_memory;
pub mod security_ops;
pub mod system_ops;

pub use channel::*;
pub use endpoints::*;
pub use remote_memory::*;
pub use security_ops::*;
pub use system_ops::*;
