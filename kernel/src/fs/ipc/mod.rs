//! IPC Filesystems - Inter-Process Communication
//!
//! Provides special filesystems for IPC mechanisms:
//! - pipefs: Named pipes (FIFOs) and anonymous pipes
//! - socketfs: Unix domain sockets
//! - shmfs: Shared memory segments
//!
//! ## Architecture
//! - Lock-free ring buffers for pipes (where possible)
//! - Wait queues for blocking operations
//! - Full POSIX semantics
//! - Support for select/poll
//!
//! ## Performance Targets
//! - Pipe throughput: > 10 GB/s
//! - Socket throughput: > 8 GB/s
//! - Shared memory: Direct memory mapping
//! - Blocking latency: < 1μs wake-up time

pub mod pipefs;
pub mod socketfs;
pub mod shmfs;

pub use pipefs::{PipeFs, PipeInode, pipe_create};
pub use socketfs::{SocketFs, SocketInode, socket_create};
pub use shmfs::{ShmFs, ShmInode, shm_create};

use crate::fs::FsResult;

/// Initialize pipe filesystem
pub fn init_pipefs() {
    pipefs::init();
    log::info!("✓ PipeFS initialized");
}

/// Initialize socket filesystem
pub fn init_socketfs() {
    socketfs::init();
    log::info!("✓ SocketFS initialized");
}

/// Initialize shared memory filesystem
pub fn init_shmfs() {
    shmfs::init();
    log::info!("✓ ShmFS initialized");
}

/// Initialize all IPC filesystems
pub fn init() {
    log::info!("Initializing IPC filesystems...");
    init_pipefs();
    init_socketfs();
    init_shmfs();
    log::info!("✓ IPC filesystems initialized");
}
