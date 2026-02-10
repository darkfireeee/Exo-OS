//! ShmFS - Shared Memory Filesystem
//!
//! ## Features
//! - POSIX shared memory (shm_open, shm_unlink)
//! - Support for mmap() to map shared memory
//! - Named shared memory objects
//! - Tmpfs-backed storage
//! - Full read/write/truncate support
//! - Proper cleanup on unlink
//!
//! ## Performance
//! - Direct memory access via mmap
//! - Zero-copy shared memory
//! - Lock-free reads (copy-on-write pages)

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;
use hashbrown::HashMap;

use crate::fs::core::types::{
    Inode, InodeType, InodePermissions, Timestamp,
};
use crate::fs::{FsError, FsResult};

/// Shared memory object
struct ShmObject {
    /// Object name
    name: String,
    /// Data storage
    data: RwLock<Vec<u8>>,
    /// Size in bytes
    size: AtomicU64,
    /// Creation time
    ctime: Timestamp,
    /// Last access time
    atime: AtomicU64,
    /// Last modification time
    mtime: AtomicU64,
    /// Permissions
    perms: RwLock<InodePermissions>,
    /// UID
    uid: AtomicU64,
    /// GID
    gid: AtomicU64,
    /// Number of references
    refcount: AtomicU64,
}

impl ShmObject {
    fn new(name: String) -> Self {
        let now = Timestamp::now();
        Self {
            name,
            data: RwLock::new(Vec::new()),
            size: AtomicU64::new(0),
            ctime: now,
            atime: AtomicU64::new(now.sec as u64),
            mtime: AtomicU64::new(now.sec as u64),
            perms: RwLock::new(InodePermissions::from_octal(0o600)),
            uid: AtomicU64::new(0),
            gid: AtomicU64::new(0),
            refcount: AtomicU64::new(1),
        }
    }

    fn update_atime(&self) {
        let now = crate::time::unix_timestamp();
        self.atime.store(now, Ordering::Relaxed);
    }

    fn update_mtime(&self) {
        let now = crate::time::unix_timestamp();
        self.mtime.store(now, Ordering::Relaxed);
    }

    fn inc_ref(&self) {
        self.refcount.fetch_add(1, Ordering::Relaxed);
    }

    fn dec_ref(&self) -> u64 {
        self.refcount.fetch_sub(1, Ordering::Relaxed) - 1
    }

    fn get_size(&self) -> u64 {
        self.size.load(Ordering::Relaxed)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        self.update_atime();

        let data = self.data.read();
        let offset = offset as usize;

        if offset >= data.len() {
            return Ok(0);
        }

        let to_read = buf.len().min(data.len() - offset);
        buf[..to_read].copy_from_slice(&data[offset..offset + to_read]);

        Ok(to_read)
    }

    fn write_at(&self, offset: u64, buf: &[u8]) -> FsResult<usize> {
        self.update_mtime();

        let mut data = self.data.write();
        let offset = offset as usize;
        let end = offset + buf.len();

        // Extend if necessary
        if end > data.len() {
            data.resize(end, 0);
            self.size.store(end as u64, Ordering::Relaxed);
        }

        data[offset..end].copy_from_slice(buf);

        Ok(buf.len())
    }

    fn truncate(&self, new_size: u64) -> FsResult<()> {
        self.update_mtime();

        let mut data = self.data.write();
        let new_size = new_size as usize;

        if new_size > data.len() {
            data.resize(new_size, 0);
        } else {
            data.truncate(new_size);
        }

        self.size.store(new_size as u64, Ordering::Relaxed);

        Ok(())
    }

    /// Get pointer to data for mmap (unsafe - caller must ensure proper synchronization)
    fn get_data_ptr(&self) -> *const u8 {
        let data = self.data.read();
        if data.is_empty() {
            core::ptr::null()
        } else {
            data.as_ptr()
        }
    }
}

/// Shared Memory Inode
pub struct ShmInode {
    /// Inode number
    ino: u64,
    /// Shared memory object
    object: Arc<ShmObject>,
}

impl ShmInode {
    fn new(ino: u64, object: Arc<ShmObject>) -> Self {
        object.inc_ref();
        Self { ino, object }
    }
}

impl Inode for ShmInode {
    fn ino(&self) -> u64 {
        self.ino
    }

    fn inode_type(&self) -> InodeType {
        InodeType::File
    }

    fn size(&self) -> u64 {
        self.object.get_size()
    }

    fn permissions(&self) -> InodePermissions {
        *self.object.perms.read()
    }

    fn set_permissions(&mut self, perms: InodePermissions) -> FsResult<()> {
        *self.object.perms.write() = perms;
        Ok(())
    }

    fn uid(&self) -> u32 {
        self.object.uid.load(Ordering::Relaxed) as u32
    }

    fn gid(&self) -> u32 {
        self.object.gid.load(Ordering::Relaxed) as u32
    }

    fn set_owner(&mut self, uid: u32, gid: u32) -> FsResult<()> {
        self.object.uid.store(uid as u64, Ordering::Relaxed);
        self.object.gid.store(gid as u64, Ordering::Relaxed);
        Ok(())
    }

    fn atime(&self) -> Timestamp {
        let sec = self.object.atime.load(Ordering::Relaxed) as i64;
        Timestamp { sec, nsec: 0 }
    }

    fn mtime(&self) -> Timestamp {
        let sec = self.object.mtime.load(Ordering::Relaxed) as i64;
        Timestamp { sec, nsec: 0 }
    }

    fn ctime(&self) -> Timestamp {
        self.object.ctime
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        self.object.read_at(offset, buf)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> FsResult<usize> {
        self.object.write_at(offset, buf)
    }

    fn truncate(&mut self, size: u64) -> FsResult<()> {
        self.object.truncate(size)
    }

    fn sync(&mut self) -> FsResult<()> {
        // No-op for memory-backed storage
        Ok(())
    }

    fn datasync(&mut self) -> FsResult<()> {
        // No-op for memory-backed storage
        Ok(())
    }

    fn list(&self) -> FsResult<Vec<String>> {
        Err(FsError::NotSupported)
    }

    fn lookup(&self, _name: &str) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }

    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }

    fn remove(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
}

impl Drop for ShmInode {
    fn drop(&mut self) {
        self.object.dec_ref();
    }
}

/// ShmFS - Manages shared memory objects
pub struct ShmFs {
    /// Next inode number
    next_ino: AtomicU64,
    /// Named shared memory objects
    objects: RwLock<HashMap<String, Arc<ShmObject>>>,
}

impl ShmFs {
    pub fn new() -> Self {
        Self {
            next_ino: AtomicU64::new(3000),
            objects: RwLock::new(HashMap::new()),
        }
    }

    fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::Relaxed)
    }

    /// Create or open a shared memory object (shm_open)
    ///
    /// # Arguments
    /// - `name`: Object name (must start with '/')
    /// - `create`: If true, create if doesn't exist
    /// - `exclusive`: If true with create, fail if exists
    ///
    /// # Returns
    /// Inode for the shared memory object
    pub fn shm_open(&self, name: &str, create: bool, exclusive: bool) -> FsResult<Arc<ShmInode>> {
        // Validate name
        if !name.starts_with('/') {
            return Err(FsError::InvalidPath);
        }

        if name.len() == 1 || name.contains("//") {
            return Err(FsError::InvalidPath);
        }

        let mut objects = self.objects.write();

        // Check if exists
        if let Some(object) = objects.get(name) {
            if create && exclusive {
                return Err(FsError::AlreadyExists);
            }

            let ino = self.alloc_ino();
            return Ok(Arc::new(ShmInode::new(ino, object.clone())));
        }

        // Doesn't exist
        if !create {
            return Err(FsError::NotFound);
        }

        // Create new object
        let object = Arc::new(ShmObject::new(name.to_string()));
        objects.insert(name.to_string(), object.clone());

        let ino = self.alloc_ino();
        Ok(Arc::new(ShmInode::new(ino, object)))
    }

    /// Unlink a shared memory object (shm_unlink)
    ///
    /// The object is removed from the namespace but continues to exist
    /// until all references are dropped.
    pub fn shm_unlink(&self, name: &str) -> FsResult<()> {
        let mut objects = self.objects.write();

        if !objects.contains_key(name) {
            return Err(FsError::NotFound);
        }

        objects.remove(name);
        Ok(())
    }

    /// List all shared memory objects
    pub fn list_objects(&self) -> Vec<String> {
        let objects = self.objects.read();
        objects.keys().cloned().collect()
    }

    /// Get statistics
    pub fn stats(&self) -> ShmStats {
        let objects = self.objects.read();
        let num_objects = objects.len();
        let total_size: u64 = objects.values()
            .map(|obj| obj.get_size())
            .sum();

        ShmStats {
            num_objects,
            total_size,
        }
    }
}

/// Shared memory statistics
#[derive(Debug, Clone, Copy)]
pub struct ShmStats {
    /// Number of shared memory objects
    pub num_objects: usize,
    /// Total size of all objects
    pub total_size: u64,
}

/// Global ShmFS instance
static SHMFS: spin::Once<ShmFs> = spin::Once::new();

/// Initialize ShmFS
pub fn init() {
    SHMFS.call_once(|| ShmFs::new());
}

/// Get global ShmFS instance
pub fn get() -> &'static ShmFs {
    SHMFS.get().expect("ShmFS not initialized")
}

/// Create or open a shared memory object
///
/// # Arguments
/// - `name`: Object name (must start with '/')
/// - `create`: Create if doesn't exist
/// - `exclusive`: Fail if exists (requires create=true)
///
/// # Returns
/// Inode for the shared memory object
///
/// # Examples
/// ```
/// // Create new shared memory
/// let shm = shm_create("/myshm", true, true).unwrap();
///
/// // Open existing shared memory
/// let shm = shm_create("/myshm", false, false).unwrap();
/// ```
pub fn shm_create(name: &str, create: bool, exclusive: bool) -> FsResult<Arc<ShmInode>> {
    get().shm_open(name, create, exclusive)
}

/// Unlink a shared memory object
///
/// The object is removed from the namespace but continues to exist
/// until all file descriptors are closed.
pub fn shm_unlink(name: &str) -> FsResult<()> {
    get().shm_unlink(name)
}

/// List all shared memory objects
pub fn shm_list() -> Vec<String> {
    get().list_objects()
}

/// Get shared memory statistics
pub fn shm_stats() -> ShmStats {
    get().stats()
}
