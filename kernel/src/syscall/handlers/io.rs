//! I/O System Call Handlers
//!
//! Handles file operations: open, close, read, write, seek, stat
//! Uses the central VFS API from crate::fs::vfs

use crate::fs::{vfs, FsError};
use crate::memory::{MemoryError, MemoryResult};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// File descriptor
pub type Fd = i32;

/// File offset
pub type Offset = i64;

/// File permissions/mode
pub type Mode = u32;

/// File flags
#[derive(Debug, Clone, Copy)]
pub struct FileFlags {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub create: bool,
    pub truncate: bool,
    pub nonblock: bool,
}

impl Default for FileFlags {
    fn default() -> Self {
        Self {
            read: true,
            write: false,
            append: false,
            create: false,
            truncate: false,
            nonblock: false,
        }
    }
}

/// File statistics
#[derive(Debug, Clone, Copy)]
pub struct FileStat {
    pub size: usize,
    pub blocks: usize,
    pub block_size: usize,
    pub inode: u64,
    pub mode: Mode,
    pub nlink: usize,
}

/// Seek whence
#[derive(Debug, Clone, Copy)]
pub enum SeekWhence {
    Start = 0,
    Current = 1,
    End = 2,
}

use alloc::collections::BTreeMap;
use spin::Mutex;

/// File descriptor entry mapping to VFS handle
#[derive(Debug, Clone)]
struct FileDescriptor {
    fd: Fd,
    vfs_handle: u64,      // VFS internal handle
    path: String,         // Path for debugging/stat
    offset: usize,        // Current offset (managed here for seek)
    flags: FileFlags,
}

static FD_TABLE: Mutex<BTreeMap<Fd, FileDescriptor>> = Mutex::new(BTreeMap::new());
static NEXT_FD: AtomicU64 = AtomicU64::new(3); // 0=stdin, 1=stdout, 2=stderr

/// VFS open flags (matching vfs/mod.rs)
const O_RDONLY: u32 = 0;
const O_WRONLY: u32 = 1;
const O_RDWR: u32 = 2;
const O_APPEND: u32 = 0o2000;

/// Convert FileFlags to VFS u32 flags
fn flags_to_vfs_flags(flags: &FileFlags) -> u32 {
    let mut vfs_flags = if flags.read && flags.write {
        O_RDWR
    } else if flags.write {
        O_WRONLY
    } else {
        O_RDONLY
    };
    
    if flags.append {
        vfs_flags |= O_APPEND;
    }
    
    vfs_flags
}

/// Convert VFS error to memory error
fn vfs_to_memory_error(e: FsError) -> MemoryError {
    match e {
        FsError::NotFound => MemoryError::NotFound,
        FsError::PermissionDenied => MemoryError::PermissionDenied,
        FsError::AlreadyExists | FsError::FileExists => MemoryError::AlreadyMapped,
        FsError::NotDirectory => MemoryError::InvalidAddress,
        FsError::IsDirectory => MemoryError::InvalidAddress,
        FsError::DirectoryNotEmpty => MemoryError::InvalidAddress,
        FsError::InvalidPath => MemoryError::InvalidAddress,
        FsError::InvalidArgument => MemoryError::InvalidParameter,
        FsError::TooManySymlinks => MemoryError::InvalidAddress,
        FsError::InvalidData => MemoryError::InvalidAddress,
        FsError::IoError => MemoryError::InternalError("IO error"),
        FsError::NotSupported => MemoryError::PermissionDenied,
        FsError::TooManyFiles => MemoryError::Mfile,
        FsError::InvalidFd => MemoryError::NotFound,
        FsError::ConnectionRefused => MemoryError::PermissionDenied,
        FsError::Again => MemoryError::InternalError("Try again"),
        FsError::Memory(mem_err) => mem_err,
    }
}

/// Open file using VFS
pub fn sys_open(path: &str, flags: FileFlags, mode: Mode) -> MemoryResult<Fd> {
    log::debug!(
        "sys_open: path={}, flags={:?}, mode={:o}",
        path,
        flags,
        mode
    );

    // Determine if file exists
    let exists = vfs::exists(path);

    // Handle file creation
    if !exists {
        if flags.create {
            // Create the file
            vfs::create_file(path).map_err(vfs_to_memory_error)?;
            log::debug!("sys_open: created file {}", path);
        } else {
            return Err(MemoryError::NotFound);
        }
    }

    // Convert flags to VFS format
    let vfs_flags = flags_to_vfs_flags(&flags);

    // Open via VFS
    let vfs_handle = vfs::open(path, vfs_flags).map_err(vfs_to_memory_error)?;

    // Handle truncation
    if flags.truncate && flags.write {
        // Truncate by writing empty content
        let _ = vfs::write(vfs_handle, &[]);
    }

    // Allocate FD
    let fd = NEXT_FD.fetch_add(1, Ordering::SeqCst) as i32;

    // Create descriptor
    let descriptor = FileDescriptor {
        fd,
        vfs_handle,
        path: String::from(path),
        offset: 0,
        flags,
    };

    FD_TABLE.lock().insert(fd, descriptor);

    log::info!("sys_open: {} -> fd={}, vfs_handle={}", path, fd, vfs_handle);
    Ok(fd)
}

/// Close file
pub fn sys_close(fd: Fd) -> MemoryResult<()> {
    log::debug!("sys_close: fd={}", fd);

    // Remove from FD table
    let mut table = FD_TABLE.lock();
    let descriptor = table.remove(&fd).ok_or(MemoryError::NotFound)?;

    // Close VFS handle
    vfs::close(descriptor.vfs_handle).map_err(vfs_to_memory_error)?;

    log::info!("sys_close: fd={}, path={}", fd, descriptor.path);
    Ok(())
}

/// Read from file
pub fn sys_read(fd: Fd, buffer: &mut [u8]) -> MemoryResult<usize> {
    log::debug!("sys_read: fd={}, len={}", fd, buffer.len());

    // Special handling for stdin
    if fd == 0 {
        // Stub: would read from console
        return Ok(0);
    }

    // Look up FD
    let mut table = FD_TABLE.lock();
    let descriptor = table.get_mut(&fd).ok_or(MemoryError::NotFound)?;

    // Check read permission
    if !descriptor.flags.read {
        return Err(MemoryError::PermissionDenied);
    }

    // Read from VFS at current offset
    let bytes_read = vfs::read_at(descriptor.vfs_handle, descriptor.offset, buffer)
        .map_err(vfs_to_memory_error)?;

    // Update offset
    descriptor.offset += bytes_read;

    log::debug!("sys_read: fd={}, read {} bytes", fd, bytes_read);
    Ok(bytes_read)
}

/// Write to file
pub fn sys_write(fd: Fd, buffer: &[u8]) -> MemoryResult<usize> {
    log::debug!("sys_write: fd={}, len={}", fd, buffer.len());

    // Special handling for stdout/stderr
    if fd == 1 || fd == 2 {
        // Write to serial console
        use crate::arch::serial;
        for &byte in buffer {
            serial::write_byte(byte);
        }
        return Ok(buffer.len());
    }

    // Look up FD
    let mut table = FD_TABLE.lock();
    let descriptor = table.get_mut(&fd).ok_or(MemoryError::NotFound)?;

    // Check write permission
    if !descriptor.flags.write {
        return Err(MemoryError::PermissionDenied);
    }

    // Handle append mode
    if descriptor.flags.append {
        // Get file size and set offset to end
        if let Ok(stat) = vfs::stat(&descriptor.path) {
            descriptor.offset = stat.size as usize;
        }
    }

    // Write to VFS at current offset
    let bytes_written = vfs::write_at(descriptor.vfs_handle, descriptor.offset, buffer)
        .map_err(vfs_to_memory_error)?;

    // Update offset
    descriptor.offset += bytes_written;

    log::debug!("sys_write: fd={}, wrote {} bytes", fd, bytes_written);
    Ok(bytes_written)
}

/// Seek in file
pub fn sys_seek(fd: Fd, offset: Offset, whence: SeekWhence) -> MemoryResult<usize> {
    log::debug!(
        "sys_seek: fd={}, offset={}, whence={:?}",
        fd,
        offset,
        whence
    );

    // Look up FD
    let mut table = FD_TABLE.lock();
    let descriptor = table.get_mut(&fd).ok_or(MemoryError::NotFound)?;

    // Get file size via VFS stat
    let file_size = vfs::stat(&descriptor.path)
        .map(|s| s.size as usize)
        .unwrap_or(0);

    // Calculate new offset based on whence
    let new_offset = match whence {
        SeekWhence::Start => offset.max(0) as usize,
        SeekWhence::Current => {
            let current = descriptor.offset as i64;
            (current + offset).max(0) as usize
        }
        SeekWhence::End => {
            let end = file_size as i64;
            (end + offset).max(0) as usize
        }
    };

    // Update file offset
    descriptor.offset = new_offset;

    log::debug!("sys_seek: fd={}, new_offset={}", fd, new_offset);
    Ok(new_offset)
}

/// Get file statistics
pub fn sys_stat(path: &str) -> MemoryResult<FileStat> {
    log::debug!("sys_stat: path={}", path);

    // Get stat via VFS
    let vfs_stat = vfs::stat(path).map_err(vfs_to_memory_error)?;

    // Get inode number via lookup
    let inode = vfs::lookup(path).unwrap_or(0);

    // Convert to our stat format
    let mode = if vfs_stat.is_dir { 0o40755 } else { 0o100644 };
    let stat = FileStat {
        size: vfs_stat.size as usize,
        blocks: (vfs_stat.size as usize + 4095) / 4096,
        block_size: 4096,
        inode,
        mode,
        nlink: 1,
    };

    Ok(stat)
}

/// Get file statistics by FD
pub fn sys_fstat(fd: Fd) -> MemoryResult<FileStat> {
    log::debug!("sys_fstat: fd={}", fd);

    // Look up FD
    let table = FD_TABLE.lock();
    let descriptor = table.get(&fd).ok_or(MemoryError::NotFound)?;

    // Get stat via path
    sys_stat(&descriptor.path)
}

/// Duplicate file descriptor
pub fn sys_dup(oldfd: Fd) -> MemoryResult<Fd> {
    log::debug!("sys_dup: fd={}", oldfd);

    // Look up FD
    let mut table = FD_TABLE.lock();
    let descriptor = table.get(&oldfd).ok_or(MemoryError::NotFound)?;

    // Reopen file via VFS (creates new handle)
    let vfs_flags = flags_to_vfs_flags(&descriptor.flags);
    let new_vfs_handle = vfs::open(&descriptor.path, vfs_flags)
        .map_err(vfs_to_memory_error)?;

    // Allocate new FD
    let new_fd = NEXT_FD.fetch_add(1, Ordering::SeqCst) as i32;

    // Copy descriptor with new FD and handle
    let new_descriptor = FileDescriptor {
        fd: new_fd,
        vfs_handle: new_vfs_handle,
        path: descriptor.path.clone(),
        offset: descriptor.offset,
        flags: descriptor.flags,
    };

    table.insert(new_fd, new_descriptor);

    log::info!("sys_dup: fd={} -> new_fd={}", oldfd, new_fd);
    Ok(new_fd)
}

/// Duplicate file descriptor to specific FD
pub fn sys_dup2(oldfd: Fd, newfd: Fd) -> MemoryResult<Fd> {
    log::debug!("sys_dup2: oldfd={}, newfd={}", oldfd, newfd);

    if oldfd == newfd {
        return Ok(newfd);
    }

    let mut table = FD_TABLE.lock();

    // Close newfd if open
    if let Some(old_desc) = table.remove(&newfd) {
        let _ = vfs::close(old_desc.vfs_handle);
    }

    // Get old descriptor
    let descriptor = table.get(&oldfd).ok_or(MemoryError::NotFound)?;

    // Reopen file via VFS
    let vfs_flags = flags_to_vfs_flags(&descriptor.flags);
    let new_vfs_handle = vfs::open(&descriptor.path, vfs_flags)
        .map_err(vfs_to_memory_error)?;

    // Create new descriptor
    let new_descriptor = FileDescriptor {
        fd: newfd,
        vfs_handle: new_vfs_handle,
        path: descriptor.path.clone(),
        offset: descriptor.offset,
        flags: descriptor.flags,
    };

    table.insert(newfd, new_descriptor);

    log::info!("sys_dup2: oldfd={} -> newfd={}", oldfd, newfd);
    Ok(newfd)
}

/// Read directory entries
pub fn sys_readdir(fd: Fd, buffer: &mut [u8]) -> MemoryResult<usize> {
    log::debug!("sys_readdir: fd={}, len={}", fd, buffer.len());

    // Look up FD
    let table = FD_TABLE.lock();
    let descriptor = table.get(&fd).ok_or(MemoryError::NotFound)?;

    // Check it's a directory
    if !vfs::is_directory(&descriptor.path) {
        return Err(MemoryError::InvalidAddress); // Not a directory
    }

    // Read directory entries via VFS (returns Vec<String>)
    let entries = vfs::readdir(&descriptor.path).map_err(vfs_to_memory_error)?;

    // Format entries into buffer (simple format: name\0name\0...)
    let mut offset = 0;
    for name in entries {
        let name_bytes = name.as_bytes();
        if offset + name_bytes.len() + 1 > buffer.len() {
            break;
        }
        buffer[offset..offset + name_bytes.len()].copy_from_slice(name_bytes);
        buffer[offset + name_bytes.len()] = 0; // null terminator
        offset += name_bytes.len() + 1;
    }

    log::debug!("sys_readdir: fd={}, returned {} bytes", fd, offset);
    Ok(offset)
}

// ============================================================================
// Additional utility functions for compatibility
// ============================================================================

/// Check if a file exists
pub fn sys_access(path: &str) -> MemoryResult<bool> {
    Ok(vfs::exists(path))
}

/// Create directory
pub fn sys_mkdir(path: &str, _mode: Mode) -> MemoryResult<()> {
    vfs::create_dir(path).map_err(vfs_to_memory_error)?;
    Ok(())
}

/// Remove directory
pub fn sys_rmdir(path: &str) -> MemoryResult<()> {
    vfs::rmdir(path).map_err(vfs_to_memory_error)
}

/// Unlink (delete) file
pub fn sys_unlink(path: &str) -> MemoryResult<()> {
    vfs::unlink(path).map_err(vfs_to_memory_error)
}

/// Rename file
pub fn sys_rename(old_path: &str, new_path: &str) -> MemoryResult<()> {
    vfs::rename(old_path, new_path).map_err(vfs_to_memory_error)
}
