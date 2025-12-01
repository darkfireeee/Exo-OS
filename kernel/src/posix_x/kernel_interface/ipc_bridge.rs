//! IPC Bridge
//!
//! Bridges POSIX IPC (pipes, etc.) to Exo-OS Fusion Rings.

use crate::fs::vfs::inode::{Inode, InodePermissions, InodeType};
use crate::fs::{FsError, FsResult};
use crate::ipc::fusion_ring::FusionRing;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Pipe Inode
///
/// Wraps a shared FusionRing.
/// One end is read-only, the other is write-only.
pub struct PipeInode {
    /// Unique inode number
    ino: u64,
    /// Shared ring buffer
    ring: Arc<FusionRing>,
    /// Is this the write end?
    is_write_end: bool,
}

impl PipeInode {
    /// Create a new pipe inode
    pub fn new(ino: u64, ring: Arc<FusionRing>, is_write_end: bool) -> Self {
        Self {
            ino,
            ring,
            is_write_end,
        }
    }
}

impl Inode for PipeInode {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn inode_type(&self) -> InodeType {
        InodeType::Fifo
    }

    fn size(&self) -> u64 {
        // For pipes, size is the number of bytes currently in the ring
        self.ring.stats().length as u64
    }

    fn permissions(&self) -> InodePermissions {
        let mut perms = InodePermissions::new();
        // Set permissions based on end type
        if self.is_write_end {
            perms.set_permissions(0o200); // Write only
        } else {
            perms.set_permissions(0o400); // Read only
        }
        perms
    }

    fn read_at(&self, _offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if self.is_write_end {
            return Err(FsError::PermissionDenied);
        }

        // Pipe reads ignore offset and consume from the ring
        // Use blocking receive for now (standard pipe behavior)
        // TODO: Handle O_NONBLOCK
        match self.ring.recv_blocking(buf) {
            Ok(n) => Ok(n),
            Err(_) => Err(FsError::IoError),
        }
    }

    fn write_at(&mut self, _offset: u64, buf: &[u8]) -> FsResult<usize> {
        if !self.is_write_end {
            return Err(FsError::PermissionDenied);
        }

        // Pipe writes ignore offset and append to the ring
        // Use blocking send for now
        match self.ring.send_blocking(buf) {
            Ok(_) => Ok(buf.len()),
            Err(_) => Err(FsError::IoError),
        }
    }

    fn truncate(&mut self, _size: u64) -> FsResult<()> {
        Err(FsError::PermissionDenied)
    }

    fn list(&self) -> FsResult<Vec<String>> {
        Err(FsError::NotDirectory)
    }

    fn lookup(&self, _name: &str) -> FsResult<u64> {
        Err(FsError::NotDirectory)
    }

    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<u64> {
        Err(FsError::NotDirectory)
    }

    fn remove(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::NotDirectory)
    }
}

/// Helper to create a new pipe pair (read, write)
pub fn create_pipe_pair(read_ino: u64, write_ino: u64) -> (Arc<PipeInode>, Arc<PipeInode>) {
    // Default capacity for pipes (e.g. 64KB)
    const PIPE_CAPACITY: usize = 65536;
    let ring = Arc::new(FusionRing::new(PIPE_CAPACITY));

    let read_end = Arc::new(PipeInode::new(read_ino, Arc::clone(&ring), false));
    let write_end = Arc::new(PipeInode::new(write_ino, ring, true));

    (read_end, write_end)
}
