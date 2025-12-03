//! Virtual File System layer
//!
//! Provides unified file operations across all filesystems.
//! High-performance implementation with:
//! - Global file handle table
//! - Path resolution with caching
//! - Zero-copy where possible

use super::{FsError, FsResult, FileMetadata};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::{Mutex, RwLock};

/// VFS inode
pub mod inode;

/// Directory entries
pub mod dentry;

/// Mount points
pub mod mount;

/// VFS cache
pub mod cache;

/// tmpfs - Temporary RAM filesystem
pub mod tmpfs;

use inode::{Inode, InodeType};
use tmpfs::{TmpFs, TmpfsInode};

// ============================================================================
// Global VFS State
// ============================================================================

/// Global tmpfs instance (root filesystem for now)
static TMPFS: RwLock<Option<TmpFs>> = RwLock::new(None);

/// Global file handle table
static FILE_HANDLES: RwLock<BTreeMap<u64, FileHandle>> = RwLock::new(BTreeMap::new());

/// Next file handle ID
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

/// File handle - represents an open file
#[derive(Clone)]
pub struct FileHandle {
    /// Inode number
    pub ino: u64,
    /// Current offset
    pub offset: u64,
    /// Open flags (O_RDONLY, O_WRONLY, O_RDWR, etc.)
    pub flags: u32,
    /// Path (for debugging)
    pub path: String,
}

/// Open flags
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;

// ============================================================================
// VFS Initialization
// ============================================================================

/// Initialize VFS
pub fn init() -> FsResult<()> {
    // Initialize tmpfs as root filesystem
    {
        let mut tmpfs = TMPFS.write();
        *tmpfs = Some(TmpFs::new());
    }
    
    cache::init();
    
    // Create standard directories
    create_dir("/bin")?;
    create_dir("/dev")?;
    create_dir("/etc")?;
    create_dir("/home")?;
    create_dir("/tmp")?;
    create_dir("/proc")?;
    create_dir("/sys")?;
    
    log::info!("VFS initialized with tmpfs root and standard directories");
    Ok(())
}

// ============================================================================
// Path Resolution
// ============================================================================

/// Resolve path to inode number
fn resolve_path(path: &str) -> FsResult<u64> {
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;
    
    // Normalize path
    let path = if path.starts_with('/') { path } else { return Err(FsError::InvalidPath); };
    
    if path == "/" {
        return Ok(1); // Root inode
    }
    
    // Split path and traverse
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut current_ino = 1u64; // Start at root
    
    for part in parts {
        let inode = fs.get_inode(current_ino)?;
        let inode_guard = inode.read();
        
        // Check if it's a directory
        if inode_guard.inode_type() != InodeType::Directory {
            return Err(FsError::NotDirectory);
        }
        
        // Lookup child
        current_ino = inode_guard.lookup(part)?;
    }
    
    Ok(current_ino)
}

/// Resolve parent directory and return (parent_ino, filename)
fn resolve_parent(path: &str) -> FsResult<(u64, String)> {
    let path = if path.starts_with('/') { path } else { return Err(FsError::InvalidPath); };
    
    if path == "/" {
        return Err(FsError::InvalidPath); // Can't get parent of root
    }
    
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err(FsError::InvalidPath);
    }
    
    let filename = parts.last().unwrap().to_string();
    
    if parts.len() == 1 {
        // Parent is root
        return Ok((1, filename));
    }
    
    // Resolve parent path
    let parent_path: String = "/".to_string() + &parts[..parts.len()-1].join("/");
    let parent_ino = resolve_path(&parent_path)?;
    
    Ok((parent_ino, filename))
}

// ============================================================================
// File Operations
// ============================================================================

/// Open a file and return handle ID
pub fn open(path: &str, flags: u32) -> FsResult<u64> {
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
            drop(tmpfs_guard); // Release read lock
            create_file(path)?
        }
        Err(e) => return Err(e),
    };
    
    // Create file handle
    let handle_id = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    let handle = FileHandle {
        ino,
        offset: if flags & O_APPEND != 0 { u64::MAX } else { 0 }, // MAX means append mode
        flags,
        path: path.to_string(),
    };
    
    FILE_HANDLES.write().insert(handle_id, handle);
    
    log::debug!("VFS: opened {} -> handle {}", path, handle_id);
    Ok(handle_id)
}

/// Close a file handle
pub fn close(handle_id: u64) -> FsResult<()> {
    FILE_HANDLES.write().remove(&handle_id).ok_or(FsError::InvalidFd)?;
    log::debug!("VFS: closed handle {}", handle_id);
    Ok(())
}

/// Read from an open file handle
pub fn read(handle_id: u64, buf: &mut [u8]) -> FsResult<usize> {
    let mut handles = FILE_HANDLES.write();
    let handle = handles.get_mut(&handle_id).ok_or(FsError::InvalidFd)?;
    
    // Check read permission
    if handle.flags & O_WRONLY != 0 {
        return Err(FsError::PermissionDenied);
    }
    
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;
    
    let inode = fs.get_inode(handle.ino)?;
    let inode_guard = inode.read();
    
    let bytes_read = inode_guard.read_at(handle.offset, buf)?;
    handle.offset += bytes_read as u64;
    
    Ok(bytes_read)
}

/// Write to an open file handle
pub fn write(handle_id: u64, buf: &[u8]) -> FsResult<usize> {
    let mut handles = FILE_HANDLES.write();
    let handle = handles.get_mut(&handle_id).ok_or(FsError::InvalidFd)?;
    
    // Check write permission
    if handle.flags & (O_WRONLY | O_RDWR) == 0 {
        return Err(FsError::PermissionDenied);
    }
    
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;
    
    let inode = fs.get_inode(handle.ino)?;
    let mut inode_guard = inode.write();
    
    // Handle append mode
    let offset = if handle.offset == u64::MAX {
        inode_guard.size()
    } else {
        handle.offset
    };
    
    let bytes_written = inode_guard.write_at(offset, buf)?;
    
    if handle.offset != u64::MAX {
        handle.offset += bytes_written as u64;
    }
    
    Ok(bytes_written)
}

/// Read from file at specific offset (without modifying handle offset)
pub fn read_at(handle_id: u64, offset: usize, buf: &mut [u8]) -> FsResult<usize> {
    let handles = FILE_HANDLES.read();
    let handle = handles.get(&handle_id).ok_or(FsError::InvalidFd)?;
    
    // Check read permission
    if handle.flags & O_WRONLY != 0 {
        return Err(FsError::PermissionDenied);
    }
    
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;
    
    let inode = fs.get_inode(handle.ino)?;
    let inode_guard = inode.read();
    
    inode_guard.read_at(offset as u64, buf)
}

/// Write to file at specific offset (without modifying handle offset)
pub fn write_at(handle_id: u64, offset: usize, buf: &[u8]) -> FsResult<usize> {
    let handles = FILE_HANDLES.read();
    let handle = handles.get(&handle_id).ok_or(FsError::InvalidFd)?;
    
    // Check write permission
    if handle.flags & (O_WRONLY | O_RDWR) == 0 {
        return Err(FsError::PermissionDenied);
    }
    
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;
    
    let inode = fs.get_inode(handle.ino)?;
    let mut inode_guard = inode.write();
    
    inode_guard.write_at(offset as u64, buf)
}

/// Seek in an open file handle
pub fn seek(handle_id: u64, offset: u64) -> FsResult<u64> {
    let mut handles = FILE_HANDLES.write();
    let handle = handles.get_mut(&handle_id).ok_or(FsError::InvalidFd)?;
    
    handle.offset = offset;
    Ok(offset)
}

/// Get file metadata from handle
pub fn fstat(handle_id: u64) -> FsResult<FileMetadata> {
    let handles = FILE_HANDLES.read();
    let handle = handles.get(&handle_id).ok_or(FsError::InvalidFd)?;
    
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;
    
    let inode = fs.get_inode(handle.ino)?;
    let inode_guard = inode.read();
    
    Ok(FileMetadata {
        size: inode_guard.size(),
        is_dir: inode_guard.inode_type() == InodeType::Directory,
        read_only: !inode_guard.permissions().user_write(),
    })
}

// ============================================================================
// High-Level File Operations (for exec() etc.)
// ============================================================================

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
    // Open or create file
    let handle = open(path, O_WRONLY | O_CREAT | O_TRUNC)?;
    
    // Write data
    write(handle, data)?;
    
    // Close
    close(handle)?;
    
    log::debug!("VFS: write_file {} -> {} bytes", path, data.len());
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
    
    // Note: inode will be freed when ref count drops to 0
    
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
        read_only: !inode_guard.permissions().user_write(),
    })
}

// ============================================================================
// Bridge functions for other modules (POSIX-X, syscalls, etc.)
// ============================================================================

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

/// Resolve path and return inode number (used by syscall handlers)
pub fn lookup(path: &str) -> FsResult<u64> {
    resolve_path(path)
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
    
    log::debug!("VFS: created symlink {} -> {}", link_path, target);
    Ok(())
}

/// Read symlink target
pub fn readlink(path: &str) -> FsResult<String> {
    let tmpfs = TMPFS.read();
    let fs = tmpfs.as_ref().ok_or(FsError::NotFound)?;
    
    let ino = resolve_path(path)?;
    let inode = fs.get_inode(ino)?;
    let inode_guard = inode.read();
    
    if inode_guard.inode_type() != InodeType::Symlink {
        return Err(FsError::InvalidArgument);
    }
    
    let mut buf = alloc::vec![0u8; 4096];
    let n = inode_guard.read_at(0, &mut buf)?;
    
    String::from_utf8(buf[..n].to_vec())
        .map_err(|_| FsError::InvalidData)
}
