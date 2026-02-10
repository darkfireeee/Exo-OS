//! VFS - Virtual File System
//!
//! Main VFS interface providing unified file operations across all filesystems.
//! Migrated from fs/vfs/mod.rs to core/vfs.rs for better organization.
//!
//! ## Architecture
//! - Lock-free dentry cache for fast path lookups
//! - Per-process file descriptor tables
//! - Mount point support
//! - Multiple filesystem backends (tmpfs, ext4plus, fat32, etc.)

use alloc::sync::Arc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use spin::{RwLock, Once};

use crate::fs::{FsError, FsResult, FileMetadata};
use super::types::*;
use super::descriptor::FileDescriptorTable;
use super::dentry::{Dentry, DentryCache};
use super::inode::InodeCache;

// Temporarily import tmpfs until we have ext4plus working
use crate::fs::compatibility::tmpfs::{TmpFs, TmpfsInode};

// Re-export fundamental types for external use (lib.rs compatibility)
pub use super::types::{Inode, InodeType, InodePermissions, Timestamp, FileHandle};

// ═══════════════════════════════════════════════════════════════════════════
// GLOBAL VFS STATE
// ═══════════════════════════════════════════════════════════════════════════

/// Global tmpfs instance (root filesystem temporarily)
/// TODO: Replace with ext4plus when ready
static TMPFS: RwLock<Option<TmpFs>> = RwLock::new(None);

/// Global file descriptor table
/// NOTE: This will be per-process when process management is complete
static GLOBAL_FD_TABLE: Once<FileDescriptorTable> = Once::new();

/// Global dentry cache
static DENTRY_CACHE: Once<DentryCache> = Once::new();

/// Global inode cache
static INODE_CACHE: Once<InodeCache> = Once::new();

/// Initialize global FD table
fn init_fd_table() {
    GLOBAL_FD_TABLE.call_once(|| FileDescriptorTable::new());
}

/// Get global FD table (temporary until per-process implementation)
fn fd_table() -> &'static FileDescriptorTable {
    GLOBAL_FD_TABLE.get().expect("FD table not initialized")
}

/// Initialize global dentry cache
fn init_dentry_cache() {
    DENTRY_CACHE.call_once(|| DentryCache::new(100_000));
}

/// Get global dentry cache
fn dentry_cache() -> &'static DentryCache {
    DENTRY_CACHE.get().expect("Dentry cache not initialized")
}

/// Initialize global inode cache
fn init_inode_cache() {
    INODE_CACHE.call_once(|| InodeCache::new(10_000));
}

/// Get global inode cache
pub fn inode_cache() -> &'static InodeCache {
    INODE_CACHE.get().expect("Inode cache not initialized")
}

// ═══════════════════════════════════════════════════════════════════════════
// VFS INITIALIZATION
// ═══════════════════════════════════════════════════════════════════════════

/// Initialize VFS
pub fn init() -> FsResult<()> {
    log::info!("Initializing VFS...");

    // Initialize global structures
    init_fd_table();
    init_dentry_cache();
    init_inode_cache();

    // Initialize tmpfs as root filesystem (temporary)
    {
        let mut tmpfs = TMPFS.write();
        *tmpfs = Some(TmpFs::new());
    }

    // Create standard directories
    create_dir("/bin")?;
    create_dir("/dev")?;
    create_dir("/etc")?;
    create_dir("/home")?;
    create_dir("/tmp")?;
    create_dir("/proc")?;
    create_dir("/sys")?;

    // Load test binaries if available
    let _ = load_test_binaries();

    log::info!("✓ VFS initialized with tmpfs root");
    Ok(())
}

/// Load test binaries into tmpfs at boot
fn load_test_binaries() -> FsResult<()> {
    // Test binaries loading is disabled for now
    // Enable when binaries are built with musl-gcc
    log::debug!("VFS: test binaries loading skipped (build with musl-gcc first)");
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// PATH RESOLUTION
// ═══════════════════════════════════════════════════════════════════════════

/// Resolve path to inode number
///
/// # Performance
/// - Cache hit: < 100ns (dentry cache)
/// - Cache miss: < 40µs (full path traversal)
fn resolve_path(path: &str) -> FsResult<u64> {
    // Normalize path
    let path = if path.starts_with('/') { 
        path 
    } else { 
        return Err(FsError::InvalidPath); 
    };

    // Root is always inode 1
    if path == "/" {
        return Ok(1);
    }

    // Check dentry cache first
    if let Some(dentry) = dentry_cache().lookup(path) {
        return Ok(dentry.ino);
    }

    // Cache miss - perform full path resolution
    let tmpfs_guard = TMPFS.read();
    let fs = tmpfs_guard.as_ref().ok_or(FsError::NotFound)?;

    let root = fs.get_inode(1)?;
    let mut current_inode = root;

    // Split path and traverse
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    for part in parts.iter() {
        let inode_guard = current_inode.read();

        // Check if it's a directory
        if inode_guard.inode_type() != InodeType::Directory {
            return Err(FsError::NotDirectory);
        }

        // Lookup child
        let child_ino = inode_guard.lookup(part)?;
        drop(inode_guard);

        // Get child inode
        current_inode = fs.get_inode(child_ino)?;
    }

    let final_ino = current_inode.read().ino();

    // Cache the result
    let dentry = Arc::new(Dentry::new(path.to_string(), final_ino, None));
    dentry_cache().insert(path.to_string(), dentry);

    Ok(final_ino)
}

/// Resolve parent directory and return (parent_ino, filename)
fn resolve_parent(path: &str) -> FsResult<(u64, String)> {
    let path = if path.starts_with('/') { 
        path 
    } else { 
        return Err(FsError::InvalidPath); 
    };

    if path == "/" {
        return Err(FsError::InvalidPath); // Can't get parent of root
    }

    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err(FsError::InvalidPath);
    }

    let filename = parts[parts.len() - 1].to_string();

    if parts.len() == 1 {
        // Parent is root
        return Ok((1, filename));
    }

    // Resolve parent path
    let parent_path: String = "/".to_string() + &parts[..parts.len()-1].join("/");
    let parent_ino = resolve_path(&parent_path)?;

    Ok((parent_ino, filename))
}

// ═══════════════════════════════════════════════════════════════════════════
// FILE DESCRIPTOR OPERATIONS
// ═══════════════════════════════════════════════════════════════════════════

/// Open a file and return file descriptor
///
/// # Arguments
/// - `path`: File path (must be absolute)
/// - `flags`: Open flags (O_RDONLY, O_WRONLY, O_RDWR, O_CREAT, O_TRUNC, etc.)
///
/// # Returns
/// File descriptor number (>= 0) or error
pub fn open(path: &str, flags: u32) -> FsResult<i32> {
    let tmpfs_guard = TMPFS.read();
    let fs = tmpfs_guard.as_ref().ok_or(FsError::NotFound)?;

    // Try to resolve existing file
    let ino = match resolve_path(path) {
        Ok(ino) => {
            // File exists
            if flags & O_EXCL != 0 && flags & O_CREAT != 0 {
                return Err(FsError::AlreadyExists);
            }

            // Truncate if requested
            if flags & O_TRUNC != 0 {
                let inode = fs.get_inode(ino)?;
                let mut inode_guard = inode.write();
                inode_guard.truncate(0)?;
            }

            ino
        }
        Err(FsError::NotFound) => {
            // File doesn't exist
            if flags & O_CREAT == 0 {
                return Err(FsError::NotFound);
            }

            // Create new file
            drop(tmpfs_guard);
            create_file(path)?
        }
        Err(e) => return Err(e),
    };

    // Create file handle
    let handle = FileHandle::new(ino, path.to_string(), flags);

    // Allocate file descriptor
    let fd = fd_table().allocate_fd(handle)? as i32;

    log::debug!("VFS: opened {} -> fd {}", path, fd);
    Ok(fd)
}

/// Close a file descriptor
pub fn close(fd: i32) -> FsResult<()> {
    fd_table().close(fd as u32)?;
    log::debug!("VFS: closed fd {}", fd);
    Ok(())
}

/// Read from an open file descriptor
///
/// # Arguments
/// - `fd`: File descriptor
/// - `buf`: Buffer to read into
///
/// # Returns
/// Number of bytes read
pub fn read(fd: i32, buf: &mut [u8]) -> FsResult<usize> {
    let handle = fd_table().get(fd as u32)?;

    // Check read permission
    if !handle.is_readable() {
        return Err(FsError::PermissionDenied);
    }

    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let inode = fs.get_inode(handle.ino)?;
    let inode_guard = inode.read();

    // Get current offset
    let offset = handle.get_offset();

    // Read from inode
    let bytes_read = inode_guard.read_at(offset, buf)?;

    // Update offset
    handle.advance_offset(bytes_read);

    Ok(bytes_read)
}

/// Write to an open file descriptor
///
/// # Arguments
/// - `fd`: File descriptor
/// - `buf`: Buffer to write from
///
/// # Returns
/// Number of bytes written
pub fn write(fd: i32, buf: &[u8]) -> FsResult<usize> {
    let handle = fd_table().get(fd as u32)?;

    // Check write permission
    if !handle.is_writable() {
        return Err(FsError::PermissionDenied);
    }

    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let inode = fs.get_inode(handle.ino)?;
    let mut inode_guard = inode.write();

    // Handle append mode
    let offset = if handle.is_append() {
        inode_guard.size()
    } else {
        handle.get_offset()
    };

    // Write to inode
    let bytes_written = inode_guard.write_at(offset, buf)?;

    // Update offset (unless in append mode)
    if !handle.is_append() {
        handle.advance_offset(bytes_written);
    }

    Ok(bytes_written)
}

/// Read from file at specific offset (pread)
pub fn read_at(fd: i32, offset: usize, buf: &mut [u8]) -> FsResult<usize> {
    let handle = fd_table().get(fd as u32)?;

    if !handle.is_readable() {
        return Err(FsError::PermissionDenied);
    }

    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let inode = fs.get_inode(handle.ino)?;
    let inode_guard = inode.read();

    inode_guard.read_at(offset as u64, buf)
}

/// Write to file at specific offset (pwrite)
pub fn write_at(fd: i32, offset: usize, buf: &[u8]) -> FsResult<usize> {
    let handle = fd_table().get(fd as u32)?;

    if !handle.is_writable() {
        return Err(FsError::PermissionDenied);
    }

    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let inode = fs.get_inode(handle.ino)?;
    let mut inode_guard = inode.write();

    inode_guard.write_at(offset as u64, buf)
}

/// Seek in an open file descriptor
pub fn seek(fd: i32, offset: u64) -> FsResult<u64> {
    let handle = fd_table().get(fd as u32)?;
    handle.set_offset(offset);
    Ok(offset)
}

/// Get file metadata from file descriptor (fstat)
pub fn fstat(fd: i32) -> FsResult<FileMetadata> {
    let handle = fd_table().get(fd as u32)?;

    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let inode = fs.get_inode(handle.ino)?;
    let inode_guard = inode.read();

    Ok(FileMetadata {
        size: inode_guard.size(),
        is_dir: inode_guard.inode_type() == InodeType::Directory,
        read_only: !inode_guard.permissions().can_write(0, 0, inode_guard.uid(), inode_guard.gid()),
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// HIGH-LEVEL FILE OPERATIONS
// ═══════════════════════════════════════════════════════════════════════════

/// Read entire file contents - used by exec()
pub fn read_file(path: &str) -> FsResult<Vec<u8>> {
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let ino = resolve_path(path)?;
    let inode = fs.get_inode(ino)?;
    let inode_guard = inode.read();

    if inode_guard.inode_type() != InodeType::File {
        return Err(FsError::IsDirectory);
    }

    let size = inode_guard.size() as usize;
    let mut data = alloc::vec![0u8; size];

    let bytes_read = inode_guard.read_at(0, &mut data)?;
    data.truncate(bytes_read);

    log::debug!("VFS: read_file {} -> {} bytes", path, bytes_read);
    Ok(data)
}

/// Write entire file contents
pub fn write_file(path: &str, data: &[u8]) -> FsResult<()> {
    log::debug!("VFS: write_file {}, {} bytes", path, data.len());

    // Open or create file
    let fd = open(path, O_WRONLY | O_CREAT | O_TRUNC)?;

    // Write data
    let bytes_written = write(fd, data)?;

    // Close
    close(fd)?;

    log::debug!("VFS: write_file {} -> wrote {} bytes", path, bytes_written);

    if bytes_written != data.len() {
        log::error!("VFS: write_file incomplete! Wrote {} but expected {}", bytes_written, data.len());
    }

    Ok(())
}

/// Create a file
pub fn create_file(path: &str) -> FsResult<u64> {
    let (parent_ino, filename) = resolve_parent(path)?;

    let tmpfs_guard = TMPFS.read();
    let fs = tmpfs_guard.as_ref().ok_or(FsError::NotFound)?;

    // Get parent directory
    let parent = fs.get_inode(parent_ino)?;

    // Create new file inode
    let new_inode = fs.create_inode(InodeType::File);
    let new_ino = new_inode.read().ino();

    // Add to parent directory
    {
        let mut parent_guard = parent.write();
        // Check if already exists
        if parent_guard.lookup(&filename).is_ok() {
            return Err(FsError::AlreadyExists);
        }
        parent_guard.children.insert(filename.clone(), new_ino);
    }

    // Invalidate dentry cache for parent
    dentry_cache().invalidate(path);

    log::debug!("VFS: created file {} (ino {})", path, new_ino);
    Ok(new_ino)
}

/// Create a directory
pub fn create_dir(path: &str) -> FsResult<u64> {
    let (parent_ino, dirname) = resolve_parent(path)?;

    let tmpfs_guard = TMPFS.read();
    let fs = tmpfs_guard.as_ref().ok_or(FsError::NotFound)?;

    // Get parent directory
    let parent = fs.get_inode(parent_ino)?;

    // Create new directory inode
    let new_inode = fs.create_inode(InodeType::Directory);
    let new_ino = new_inode.read().ino();

    // Add to parent directory
    {
        let mut parent_guard = parent.write();
        // Check if already exists
        if parent_guard.lookup(&dirname).is_ok() {
            return Err(FsError::AlreadyExists);
        }
        parent_guard.children.insert(dirname.clone(), new_ino);
    }

    // Invalidate dentry cache
    dentry_cache().invalidate(path);

    log::debug!("VFS: created directory {} (ino {})", path, new_ino);
    Ok(new_ino)
}

/// Delete a file
pub fn unlink(path: &str) -> FsResult<()> {
    let (parent_ino, filename) = resolve_parent(path)?;

    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    // Get target inode first to check type
    let target_ino = resolve_path(path)?;
    let target = fs.get_inode(target_ino)?;

    if target.read().inode_type() == InodeType::Directory {
        return Err(FsError::IsDirectory);
    }

    // Remove from parent
    let parent = fs.get_inode(parent_ino)?;
    parent.write().children.remove(&filename);

    // Invalidate dentry cache
    dentry_cache().invalidate(path);

    log::debug!("VFS: unlinked {}", path);
    Ok(())
}

/// Delete an empty directory
pub fn rmdir(path: &str) -> FsResult<()> {
    let (parent_ino, dirname) = resolve_parent(path)?;

    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    // Get target inode
    let target_ino = resolve_path(path)?;
    let target = fs.get_inode(target_ino)?;
    let target_guard = target.read();

    if target_guard.inode_type() != InodeType::Directory {
        return Err(FsError::NotDirectory);
    }

    if !target_guard.children.is_empty() {
        return Err(FsError::DirectoryNotEmpty);
    }

    drop(target_guard);

    // Remove from parent
    let parent = fs.get_inode(parent_ino)?;
    parent.write().children.remove(&dirname);

    // Invalidate dentry cache
    dentry_cache().invalidate_tree(path);

    log::debug!("VFS: removed directory {}", path);
    Ok(())
}

/// List directory contents
pub fn readdir(path: &str) -> FsResult<Vec<String>> {
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let ino = resolve_path(path)?;
    let inode = fs.get_inode(ino)?;
    let inode_guard = inode.read();

    inode_guard.list()
}

/// Rename/move a file or directory
pub fn rename(old_path: &str, new_path: &str) -> FsResult<()> {
    let (old_parent_ino, old_name) = resolve_parent(old_path)?;
    let (new_parent_ino, new_name) = resolve_parent(new_path)?;

    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    // Get the inode number of the file to move
    let target_ino = resolve_path(old_path)?;

    // Remove from old parent
    {
        let old_parent = fs.get_inode(old_parent_ino)?;
        old_parent.write().children.remove(&old_name);
    }

    // Add to new parent
    {
        let new_parent = fs.get_inode(new_parent_ino)?;
        new_parent.write().children.insert(new_name.clone(), target_ino);
    }

    // Invalidate dentry cache
    dentry_cache().invalidate(old_path);
    dentry_cache().invalidate(new_path);

    log::debug!("VFS: renamed {} -> {}", old_path, new_path);
    Ok(())
}

/// Create a symlink
pub fn symlink(target: &str, link_path: &str) -> FsResult<()> {
    let (parent_ino, link_name) = resolve_parent(link_path)?;

    let tmpfs_guard = TMPFS.read();
    let fs = tmpfs_guard.as_ref().ok_or(FsError::NotFound)?;

    // Create symlink inode
    let link_inode = fs.create_inode(InodeType::Symlink);
    let link_ino = link_inode.read().ino();

    // Write target path as symlink content
    link_inode.write().write_at(0, target.as_bytes())?;

    // Add to parent directory
    {
        let parent = fs.get_inode(parent_ino)?;
        let mut parent_guard = parent.write();
        if parent_guard.lookup(&link_name).is_ok() {
            return Err(FsError::AlreadyExists);
        }
        parent_guard.children.insert(link_name, link_ino);
    }

    // Invalidate cache
    dentry_cache().invalidate(link_path);

    log::debug!("VFS: created symlink {} -> {}", link_path, target);
    Ok(())
}

/// Read symlink target
pub fn readlink(path: &str) -> FsResult<String> {
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let ino = resolve_path(path)?;
    let inode = fs.get_inode(ino)?;

    let result = inode.read().readlink();
    result
}

// ═══════════════════════════════════════════════════════════════════════════
// PATH/METADATA QUERY OPERATIONS
// ═══════════════════════════════════════════════════════════════════════════

/// Check if path exists
pub fn exists(path: &str) -> bool {
    resolve_path(path).is_ok()
}

/// Get file metadata by path
pub fn stat(path: &str) -> FsResult<FileMetadata> {
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let ino = resolve_path(path)?;
    let inode = fs.get_inode(ino)?;
    let inode_guard = inode.read();

    Ok(FileMetadata {
        size: inode_guard.size(),
        is_dir: inode_guard.inode_type() == InodeType::Directory,
        read_only: (inode_guard.permissions().0 & 0o200) == 0,
    })
}

/// Get inode type for a path
pub fn get_inode_type(path: &str) -> FsResult<InodeType> {
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;

    let ino = resolve_path(path)?;
    let inode = fs.get_inode(ino)?;
    let inode_type = inode.read().inode_type();
    Ok(inode_type)
}

/// Check if path is a directory
pub fn is_directory(path: &str) -> bool {
    get_inode_type(path).map(|t| t == InodeType::Directory).unwrap_or(false)
}

/// Check if path is a regular file
pub fn is_file(path: &str) -> bool {
    get_inode_type(path).map(|t| t == InodeType::File).unwrap_or(false)
}

// ═══════════════════════════════════════════════════════════════════════════
// BRIDGE FUNCTIONS (for other modules)
// ═══════════════════════════════════════════════════════════════════════════

/// Resolve path and return inode number (used by syscall handlers)
pub fn lookup(path: &str) -> FsResult<u64> {
    resolve_path(path)
}

/// Get inode by number (used by posix_x/vfs_posix)
pub fn get_inode(ino: u64) -> FsResult<Arc<RwLock<TmpfsInode>>> {
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;
    fs.get_inode(ino)
}

/// Get root inode (ino = 1)
pub fn get_root_inode() -> FsResult<Arc<RwLock<TmpfsInode>>> {
    get_inode(1)
}
