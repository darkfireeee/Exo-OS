//! FIFO (Named Pipe) System Calls
//!
//! Implements `mkfifo`, `mknod`.

use crate::fs::vfs::inode::InodeType;
use crate::fs::FsError;
use crate::posix_x::vfs_posix::path_resolver;

/// Create a FIFO (named pipe)
pub fn sys_mkfifo(path: &str, mode: u32) -> i32 {
    sys_mknod(path, mode | 0o010000, 0) // S_IFIFO
}

/// Create a filesystem node (file, device, FIFO)
pub fn sys_mknod(path: &str, mode: u32, dev: u64) -> i32 {
    let inode_type = if (mode & 0o010000) == 0o010000 {
        InodeType::Fifo
    } else if (mode & 0o020000) == 0o020000 {
        InodeType::CharDevice
    } else if (mode & 0o060000) == 0o060000 {
        InodeType::BlockDevice
    } else if (mode & 0o140000) == 0o140000 {
        InodeType::Socket
    } else {
        InodeType::File // Default to regular file
    };

    // TODO: Use `dev` for device nodes
    let _ = dev;

    match path_resolver::resolve_parent(path) {
        Ok(_) => 0, // Stub: In real implementation, we would create the node
        Err(e) => -(e.to_errno() as i32),
    }
}
