//! I/O System Call Handlers
//!
//! Handles file operations: open, close, read, write, seek, stat

use crate::memory::{MemoryResult, MemoryError};
use core::sync::atomic::{AtomicU64, Ordering};

/// File descriptor
pub type Fd = u64;

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

#[derive(Debug, Clone)]
struct FileDescriptor {
    fd: Fd,
    inode: u64,
    offset: usize,
    flags: FileFlags,
}

static FD_TABLE: Mutex<BTreeMap<Fd, FileDescriptor>> = Mutex::new(BTreeMap::new());
static NEXT_FD: AtomicU64 = AtomicU64::new(3); // 0=stdin, 1=stdout, 2=stderr

/// Open file
pub fn sys_open(path: &str, flags: FileFlags, mode: Mode) -> MemoryResult<Fd> {
    log::debug!("sys_open: path={}, flags={:?}, mode={:o}", path, flags, mode);
    
    // 1. Resolve path using VFS cache
    let cache = crate::fs::vfs::cache::get_cache();
    let inode = if let Some(ino) = cache.dentry_cache.lookup(path) {
        ino
    } else {
        // Create new file if create flag set
        if flags.create {
            // Stub: would create through VFS
            let new_ino = 100; // Dummy inode
            cache.dentry_cache.insert(alloc::string::String::from(path), new_ino);
            new_ino
        } else {
            return Err(MemoryError::NotFound);
        }
    };
    
    // 2. Check permissions (stub)
    
    // 3. Allocate FD
    let fd = NEXT_FD.fetch_add(1, Ordering::SeqCst);
    
    // 4. Add to process FD table
    let descriptor = FileDescriptor {
        fd,
        inode,
        offset: 0,
        flags,
    };
    
    FD_TABLE.lock().insert(fd, descriptor);
    
    log::info!("open: {} -> fd={}, inode={}", path, fd, inode);
    Ok(fd)
}

/// Close file
pub fn sys_close(fd: Fd) -> MemoryResult<()> {
    log::debug!("sys_close: fd={}", fd);
    
    // 1. Remove from FD table
    let mut table = FD_TABLE.lock();
    let descriptor = table.remove(&fd)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Decrement file ref count (stub)
    // 3. Close if ref count reaches 0 (stub)
    
    log::info!("close: fd={}, inode={}", fd, descriptor.inode);
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
    
    // 1. Look up FD
    let mut table = FD_TABLE.lock();
    let descriptor = table.get_mut(&fd)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Check read permission
    if !descriptor.flags.read {
        return Err(MemoryError::PermissionDenied);
    }
    
    // 3. Read from VFS inode (stub)
    let cache = crate::fs::vfs::cache::get_cache();
    let bytes_read = if let Some(_inode) = cache.inode_cache.get(descriptor.inode) {
        // Would call inode.read_at(offset, buffer)
        0 // Stub
    } else {
        0
    };
    
    // 4. Update offset
    descriptor.offset += bytes_read;
    
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
    
    // 1. Look up FD
    let mut table = FD_TABLE.lock();
    let descriptor = table.get_mut(&fd)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Check write permission
    if !descriptor.flags.write {
        return Err(MemoryError::PermissionDenied);
    }
    
    // 3. Write to VFS inode
    let cache = crate::fs::vfs::cache::get_cache();
    let bytes_written = if let Some(_inode) = cache.inode_cache.get(descriptor.inode) {
        // Would call inode.write_at(offset, buffer)
        buffer.len() // Stub
    } else {
        return Err(MemoryError::NotFound);
    };
    
    // 4. Update offset
    descriptor.offset += bytes_written;
    
    // Mark inode as dirty
    cache.inode_cache.mark_dirty(descriptor.inode);
    
    Ok(bytes_written)
}

/// Seek in file
pub fn sys_seek(fd: Fd, offset: Offset, whence: SeekWhence) -> MemoryResult<usize> {
    log::debug!("sys_seek: fd={}, offset={}, whence={:?}", fd, offset, whence);
    
    // 1. Look up FD
    let mut table = FD_TABLE.lock();
    let descriptor = table.get_mut(&fd)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Get file size (stub - would query inode)
    let file_size = 0usize; // Stub
    
    // 3. Calculate new offset based on whence
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
    
    // 4. Update file offset
    descriptor.offset = new_offset;
    
    log::debug!("seek: fd={}, new_offset={}", fd, new_offset);
    Ok(new_offset)
}

/// Get file statistics
pub fn sys_stat(path: &str) -> MemoryResult<FileStat> {
    log::debug!("sys_stat: path={}", path);
    
    // 1. Resolve path
    let cache = crate::fs::vfs::cache::get_cache();
    let inode = cache.dentry_cache.lookup(path)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Get inode and fill stat
    let stat = FileStat {
        size: 0,        // Would query inode.size()
        blocks: 0,
        block_size: 4096,
        inode,
        mode: 0o644,
        nlink: 1,
    };
    
    Ok(stat)
}

/// Get file statistics by FD
pub fn sys_fstat(fd: Fd) -> MemoryResult<FileStat> {
    log::debug!("sys_fstat: fd={}", fd);
    
    // 1. Look up FD
    let table = FD_TABLE.lock();
    let descriptor = table.get(&fd)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Fill stat structure
    let stat = FileStat {
        size: 0,        // Would query inode
        blocks: 0,
        block_size: 4096,
        inode: descriptor.inode,
        mode: 0o644,
        nlink: 1,
    };
    
    Ok(stat)
}

/// Duplicate file descriptor
pub fn sys_dup(fd: Fd) -> MemoryResult<Fd> {
    log::debug!("sys_dup: fd={}", fd);
    
    // 1. Look up FD
    let mut table = FD_TABLE.lock();
    let descriptor = table.get(&fd)
        .ok_or(MemoryError::NotFound)?
        .clone();
    
    // 2. Allocate new FD
    let new_fd = NEXT_FD.fetch_add(1, Ordering::SeqCst);
    
    // 3. Copy file pointer with new FD
    let mut new_descriptor = descriptor;
    new_descriptor.fd = new_fd;
    
    // 4. Insert and increment ref count (stub)
    table.insert(new_fd, new_descriptor);
    
    log::info!("dup: fd={} -> new_fd={}", fd, new_fd);
    Ok(new_fd)
}

/// Duplicate file descriptor to specific FD
pub fn sys_dup2(oldfd: Fd, newfd: Fd) -> MemoryResult<Fd> {
    log::debug!("sys_dup2: oldfd={}, newfd={}", oldfd, newfd);
    
    if oldfd == newfd {
        return Ok(newfd);
    }
    
    let mut table = FD_TABLE.lock();
    
    // 1. Close newfd if open
    if table.contains_key(&newfd) {
        table.remove(&newfd);
    }
    
    // 2. Copy oldfd to newfd
    let descriptor = table.get(&oldfd)
        .ok_or(MemoryError::NotFound)?
        .clone();
    
    let mut new_descriptor = descriptor;
    new_descriptor.fd = newfd;
    
    // 3. Insert and increment ref count
    table.insert(newfd, new_descriptor);
    
    log::info!("dup2: oldfd={} -> newfd={}", oldfd, newfd);
    Ok(newfd)
}

/// Read directory entries
pub fn sys_readdir(fd: Fd, buffer: &mut [u8]) -> MemoryResult<usize> {
    log::debug!("sys_readdir: fd={}, len={}", fd, buffer.len());
    
    // 1. Look up FD
    let table = FD_TABLE.lock();
    let descriptor = table.get(&fd)
        .ok_or(MemoryError::NotFound)?;
    
    // 2. Check it's a directory (stub - would check inode type)
    // 3. Read directory entries (stub)
    // 4. Fill buffer (stub)
    
    log::debug!("readdir: fd={}, inode={}", fd, descriptor.inode);
    Ok(0)
}
