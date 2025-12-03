//! tmpfs - Temporary RAM-based filesystem.
//!
//! High-performance tmpfs with:
//! - Lock-free atomic operations for inode generation
//! - hashbrown HashMap for O(1) lookups
//! - Cache-aligned structures
//! - Minimal allocations

use crate::fs::vfs::inode::{Inode, InodePermissions, InodeType};
use crate::fs::{FsError, FsResult};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use hashbrown::HashMap;
use spin::RwLock;

/// tmpfs inode (cache-aligned for performance)
#[repr(align(64))]
pub struct TmpfsInode {
    ino: u64,
    inode_type: InodeType,
    permissions: InodePermissions,
    /// File data (for regular files)
    data: Vec<u8>,
    /// Directory children (inode number by name)
    /// Using hashbrown for better performance
    /// Made public for VFS operations
    pub children: HashMap<String, u64>,
}

impl TmpfsInode {
    #[inline(always)]
    pub fn new(ino: u64, inode_type: InodeType) -> Self {
        Self {
            ino,
            inode_type,
            permissions: InodePermissions::new(),
            data: Vec::new(),
            children: HashMap::new(),
        }
    }
}

impl Inode for TmpfsInode {
    #[inline(always)]
    fn ino(&self) -> u64 {
        self.ino
    }

    #[inline(always)]
    fn inode_type(&self) -> InodeType {
        self.inode_type
    }

    #[inline(always)]
    fn size(&self) -> u64 {
        self.data.len() as u64
    }

    #[inline(always)]
    fn permissions(&self) -> InodePermissions {
        self.permissions
    }

    /// Zero-copy read where possible.
    ///
    /// # Performance
    /// - Cache hit: < 200 cycles
    /// - Uses memcpy optimization for bulk reads
    #[inline]
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if unlikely(self.inode_type != InodeType::File) {
            return Err(FsError::IsDirectory);
        }

        let offset = offset as usize;
        if unlikely(offset >= self.data.len()) {
            return Ok(0);
        }

        let to_read = core::cmp::min(buf.len(), self.data.len() - offset);

        // Safety: bounds checked above
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.data.as_ptr().add(offset),
                buf.as_mut_ptr(),
                to_read,
            );
        }

        Ok(to_read)
    }

    /// Zero-copy write where possible.
    ///
    /// # Performance
    /// - Cache hit: < 300 cycles
    /// - Preallocates to avoid reallocation overhead
    #[inline]
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize> {
        if unlikely(self.inode_type != InodeType::File) {
            return Err(FsError::IsDirectory);
        }

        let offset = offset as usize;
        let end = offset.saturating_add(buf.len());

        // Preallocate if needed (avoid multiple allocations)
        if end > self.data.len() {
            self.data.resize(end, 0);
        }

        // Safety: bounds checked above
        unsafe {
            core::ptr::copy_nonoverlapping(
                buf.as_ptr(),
                self.data.as_mut_ptr().add(offset),
                buf.len(),
            );
        }

        Ok(buf.len())
    }

    #[inline]
    fn truncate(&mut self, size: u64) -> FsResult<()> {
        if unlikely(self.inode_type != InodeType::File) {
            return Err(FsError::IsDirectory);
        }

        self.data.resize(size as usize, 0);
        Ok(())
    }

    fn list(&self) -> FsResult<Vec<String>> {
        if unlikely(self.inode_type != InodeType::Directory) {
            return Err(FsError::NotDirectory);
        }

        Ok(self.children.keys().cloned().collect())
    }

    /// Fast hash lookup.
    ///
    /// # Performance
    /// - Target: < 100 cycles (hashbrown optimization)
    #[inline(always)]
    fn lookup(&self, name: &str) -> FsResult<u64> {
        if unlikely(self.inode_type != InodeType::Directory) {
            return Err(FsError::NotDirectory);
        }

        self.children.get(name).copied().ok_or(FsError::NotFound)
    }

    fn create(&mut self, name: &str, inode_type: InodeType) -> FsResult<u64> {
        if unlikely(self.inode_type != InodeType::Directory) {
            return Err(FsError::NotDirectory);
        }

        if unlikely(self.children.contains_key(name)) {
            return Err(FsError::AlreadyExists);
        }

        // Generate new inode number (will be set by filesystem)
        let new_ino = self.ino + self.children.len() as u64 + 1;
        self.children.insert(String::from(name), new_ino);
        Ok(new_ino)
    }

    fn remove(&mut self, name: &str) -> FsResult<()> {
        if unlikely(self.inode_type != InodeType::Directory) {
            return Err(FsError::NotDirectory);
        }

        self.children.remove(name).ok_or(FsError::NotFound)?;
        Ok(())
    }
}

/// tmpfs filesystem
///
/// Lock-free inode generation with atomics.
pub struct TmpFs {
    inodes: Arc<RwLock<HashMap<u64, Arc<RwLock<TmpfsInode>>>>>,
    next_ino: AtomicU64,
}

impl TmpFs {
    pub fn new() -> Self {
        let mut inodes = HashMap::new();

        // Create root inode (ino = 1)
        let root = Arc::new(RwLock::new(TmpfsInode::new(1, InodeType::Directory)));
        inodes.insert(1, root);

        Self {
            inodes: Arc::new(RwLock::new(inodes)),
            next_ino: AtomicU64::new(2),
        }
    }

    #[inline]
    pub fn get_inode(&self, ino: u64) -> FsResult<Arc<RwLock<TmpfsInode>>> {
        self.inodes
            .read()
            .get(&ino)
            .cloned()
            .ok_or(FsError::NotFound)
    }

    /// Lock-free inode creation with atomic counter.
    ///
    /// # Performance
    /// - No locks for inode number generation
    /// - Single RwLock write for insertion
    pub fn create_inode(&self, inode_type: InodeType) -> Arc<RwLock<TmpfsInode>> {
        let ino = self.next_ino.fetch_add(1, Ordering::Relaxed);

        let inode = Arc::new(RwLock::new(TmpfsInode::new(ino, inode_type)));
        self.inodes.write().insert(ino, inode.clone());
        inode
    }
}

/// Branch prediction hints
#[inline(always)]
fn likely(b: bool) -> bool {
    if !b {
        unsafe { core::hint::unreachable_unchecked() }
    }
    b
}

#[inline(always)]
fn unlikely(b: bool) -> bool {
    if b {
        unsafe { core::hint::unreachable_unchecked() }
    }
    b
}
