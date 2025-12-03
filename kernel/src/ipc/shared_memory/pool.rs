//! Shared Memory Pool Management
//!
//! High-performance global pool for shared memory regions.
//! Integrates with the physical frame allocator for real memory allocation.
//!
//! Performance targets:
//! - Allocation: ~100-200 cycles (vs Linux shmget ~2000+ cycles)
//! - Lookup: O(log n) via BTreeMap
//! - Attach/Detach: ~50 cycles (atomic refcount)

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::memory::{MemoryResult, MemoryError};
use crate::memory::address::PhysicalAddress;
use crate::memory::physical::{self, Frame, FRAME_SIZE};

/// Shared memory ID - globally unique identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShmId(pub u64);

impl ShmId {
    /// Create new ID from raw value
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
    
    /// Get raw ID value
    pub const fn raw(&self) -> u64 {
        self.0
    }
}

/// Shared memory permissions with UNIX-like semantics
#[derive(Debug, Clone, Copy)]
pub struct ShmPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    /// Owner UID (for access control)
    pub owner_uid: u32,
    /// Mode bits (like 0644)
    pub mode: u16,
}

impl ShmPermissions {
    pub const READ_ONLY: Self = Self { 
        read: true, write: false, execute: false, 
        owner_uid: 0, mode: 0o444 
    };
    pub const READ_WRITE: Self = Self { 
        read: true, write: true, execute: false,
        owner_uid: 0, mode: 0o644
    };
    pub const READ_EXEC: Self = Self { 
        read: true, write: false, execute: true,
        owner_uid: 0, mode: 0o555
    };
    
    /// Create with specific owner and mode
    pub const fn with_owner(mut self, uid: u32, mode: u16) -> Self {
        self.owner_uid = uid;
        self.mode = mode;
        self
    }
    
    /// Check if user can read
    pub fn can_read(&self, uid: u32) -> bool {
        if uid == 0 { return true; } // root
        if uid == self.owner_uid { return self.mode & 0o400 != 0; }
        self.mode & 0o004 != 0
    }
    
    /// Check if user can write
    pub fn can_write(&self, uid: u32) -> bool {
        if uid == 0 { return true; }
        if uid == self.owner_uid { return self.mode & 0o200 != 0; }
        self.mode & 0o002 != 0
    }
}

/// Physical frame list for a shared memory region
struct FrameList {
    /// Starting physical address
    start_addr: PhysicalAddress,
    /// Number of frames
    frame_count: usize,
    /// Whether frames are contiguous
    contiguous: bool,
    /// Individual frames (only used if not contiguous)
    frames: Option<Vec<Frame>>,
}

impl FrameList {
    /// Create contiguous frame list
    fn contiguous(start: PhysicalAddress, count: usize) -> Self {
        Self {
            start_addr: start,
            frame_count: count,
            contiguous: true,
            frames: None,
        }
    }
    
    /// Create non-contiguous frame list
    fn scattered(frames: Vec<Frame>) -> Self {
        let start = frames.first().map(|f| f.address()).unwrap_or(PhysicalAddress::new(0));
        Self {
            start_addr: start,
            frame_count: frames.len(),
            contiguous: false,
            frames: Some(frames),
        }
    }
    
    /// Get physical address for offset
    fn phys_for_offset(&self, offset: usize) -> Option<PhysicalAddress> {
        let frame_idx = offset / FRAME_SIZE;
        if frame_idx >= self.frame_count {
            return None;
        }
        
        if self.contiguous {
            Some(PhysicalAddress::new(self.start_addr.value() + frame_idx * FRAME_SIZE))
        } else {
            self.frames.as_ref()?.get(frame_idx).map(|f| f.address())
        }
    }
}

/// Shared memory region descriptor
pub struct ShmRegion {
    /// Unique ID
    pub id: ShmId,
    
    /// Physical frames backing this region
    frames: FrameList,
    
    /// Size in bytes
    pub size: usize,
    
    /// Permissions
    pub perms: ShmPermissions,
    
    /// Owner process ID
    pub owner_pid: usize,
    
    /// Reference count (atomic for lock-free attach/detach)
    ref_count: AtomicUsize,
    
    /// Optional name for POSIX shm_open compatibility
    pub name: Option<String>,
    
    /// Creation timestamp (TSC cycles)
    pub created_at: u64,
    
    /// Last access timestamp
    pub last_access: AtomicU64,
}

impl ShmRegion {
    /// Create new region with allocated physical memory
    pub fn new(
        id: ShmId, 
        frames: FrameList, 
        size: usize, 
        perms: ShmPermissions, 
        owner: usize
    ) -> Self {
        let now = read_tsc();
        Self {
            id,
            frames,
            size,
            perms,
            owner_pid: owner,
            ref_count: AtomicUsize::new(1),
            name: None,
            created_at: now,
            last_access: AtomicU64::new(now),
        }
    }
    
    /// Get starting physical address
    pub fn phys_addr(&self) -> PhysicalAddress {
        self.frames.start_addr
    }
    
    /// Get physical address for offset into region
    pub fn phys_for_offset(&self, offset: usize) -> Option<PhysicalAddress> {
        self.frames.phys_for_offset(offset)
    }
    
    /// Get frame count
    pub fn frame_count(&self) -> usize {
        self.frames.frame_count
    }
    
    pub fn with_name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }
    
    /// Increment reference count (lock-free)
    pub fn inc_ref(&self) -> usize {
        self.last_access.store(read_tsc(), Ordering::Relaxed);
        self.ref_count.fetch_add(1, Ordering::AcqRel) + 1
    }
    
    /// Decrement reference count (lock-free), returns true if should be freed
    pub fn dec_ref(&self) -> bool {
        let prev = self.ref_count.fetch_sub(1, Ordering::AcqRel);
        prev == 1
    }
    
    /// Get current reference count
    pub fn ref_count(&self) -> usize {
        self.ref_count.load(Ordering::Acquire)
    }
    
    /// Check if region is contiguous
    pub fn is_contiguous(&self) -> bool {
        self.frames.contiguous
    }
}

/// Read TSC for timestamps
#[inline]
fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

/// Global shared memory pool
pub struct SharedMemoryPool {
    /// All regions indexed by ID
    regions: BTreeMap<ShmId, ShmRegion>,
    
    /// Named regions for POSIX shm_open() compatibility
    named: BTreeMap<String, ShmId>,
    
    /// Next available ID (atomic for potential concurrent allocation)
    next_id: AtomicU64,
    
    /// Statistics
    stats: PoolStats,
}

/// Pool statistics
#[derive(Debug, Default)]
pub struct PoolStats {
    pub total_allocated: AtomicU64,
    pub total_freed: AtomicU64,
    pub current_regions: AtomicUsize,
    pub total_bytes_allocated: AtomicU64,
    pub allocation_failures: AtomicU64,
}

impl SharedMemoryPool {
    pub const fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
            named: BTreeMap::new(),
            next_id: AtomicU64::new(1),
            stats: PoolStats {
                total_allocated: AtomicU64::new(0),
                total_freed: AtomicU64::new(0),
                current_regions: AtomicUsize::new(0),
                total_bytes_allocated: AtomicU64::new(0),
                allocation_failures: AtomicU64::new(0),
            },
        }
    }
    
    /// Generate next unique ID
    fn next_id(&self) -> ShmId {
        ShmId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }
    
    /// Allocate new shared memory region with REAL physical memory
    pub fn allocate(&mut self, size: usize, perms: ShmPermissions, owner: usize) -> MemoryResult<ShmId> {
        if size == 0 {
            return Err(MemoryError::InvalidSize);
        }
        
        // Round up to frame boundary
        let frame_count = (size + FRAME_SIZE - 1) / FRAME_SIZE;
        
        // Try contiguous allocation first (better for large regions)
        let frames = if frame_count > 1 {
            match physical::allocate_contiguous_frames(frame_count) {
                Ok(start_frame) => {
                    // Zero the memory for security
                    unsafe {
                        let ptr = start_frame.address().value() as *mut u8;
                        core::ptr::write_bytes(ptr, 0, frame_count * FRAME_SIZE);
                    }
                    FrameList::contiguous(start_frame.address(), frame_count)
                }
                Err(_) => {
                    // Fall back to scattered allocation
                    self.allocate_scattered_frames(frame_count)?
                }
            }
        } else {
            // Single frame
            let frame = physical::allocate_frame().map_err(|_| {
                self.stats.allocation_failures.fetch_add(1, Ordering::Relaxed);
                MemoryError::OutOfMemory
            })?;
            
            // Zero the frame
            unsafe {
                let ptr = frame.address().value() as *mut u8;
                core::ptr::write_bytes(ptr, 0, FRAME_SIZE);
            }
            
            FrameList::contiguous(frame.address(), 1)
        };
        
        let id = self.next_id();
        let region = ShmRegion::new(id, frames, size, perms, owner);
        
        // Update statistics
        self.stats.total_allocated.fetch_add(1, Ordering::Relaxed);
        self.stats.current_regions.fetch_add(1, Ordering::Relaxed);
        self.stats.total_bytes_allocated.fetch_add(size as u64, Ordering::Relaxed);
        
        self.regions.insert(id, region);
        
        log::debug!(
            "Allocated shared memory region {}: {} bytes ({} frames)", 
            id.0, size, frame_count
        );
        
        Ok(id)
    }
    
    /// Allocate scattered (non-contiguous) frames
    fn allocate_scattered_frames(&mut self, count: usize) -> MemoryResult<FrameList> {
        let mut frames = Vec::with_capacity(count);
        
        for _ in 0..count {
            match physical::allocate_frame() {
                Ok(frame) => {
                    // Zero each frame
                    unsafe {
                        let ptr = frame.address().value() as *mut u8;
                        core::ptr::write_bytes(ptr, 0, FRAME_SIZE);
                    }
                    frames.push(frame);
                }
                Err(_) => {
                    // Rollback: free already allocated frames
                    for f in frames {
                        let _ = physical::deallocate_frame(f);
                    }
                    self.stats.allocation_failures.fetch_add(1, Ordering::Relaxed);
                    return Err(MemoryError::OutOfMemory);
                }
            }
        }
        
        Ok(FrameList::scattered(frames))
    }
    
    /// Create named shared memory (POSIX shm_open compatible)
    pub fn create_named(
        &mut self, 
        name: String, 
        size: usize, 
        perms: ShmPermissions, 
        owner: usize
    ) -> MemoryResult<ShmId> {
        // Check if name already exists
        if self.named.contains_key(&name) {
            return Err(MemoryError::AlreadyMapped);
        }
        
        let id = self.allocate(size, perms, owner)?;
        
        if let Some(region) = self.regions.get_mut(&id) {
            region.name = Some(name.clone());
        }
        
        self.named.insert(name, id);
        Ok(id)
    }
    
    /// Open existing named shared memory
    pub fn open_named(&mut self, name: &str) -> MemoryResult<ShmId> {
        self.named.get(name)
            .copied()
            .ok_or(MemoryError::NotFound)
    }
    
    /// Get region by ID (immutable)
    pub fn get(&self, id: ShmId) -> Option<&ShmRegion> {
        self.regions.get(&id)
    }
    
    /// Get mutable region
    pub fn get_mut(&mut self, id: ShmId) -> Option<&mut ShmRegion> {
        self.regions.get_mut(&id)
    }
    
    /// Attach to shared memory (lock-free refcount increment)
    pub fn attach(&self, id: ShmId) -> MemoryResult<PhysicalAddress> {
        let region = self.regions.get(&id)
            .ok_or(MemoryError::NotFound)?;
        
        region.inc_ref();
        Ok(region.phys_addr())
    }
    
    /// Attach with permission check
    pub fn attach_checked(&self, id: ShmId, uid: u32, need_write: bool) -> MemoryResult<PhysicalAddress> {
        let region = self.regions.get(&id)
            .ok_or(MemoryError::NotFound)?;
        
        // Permission check
        if need_write && !region.perms.can_write(uid) {
            return Err(MemoryError::PermissionDenied);
        }
        if !region.perms.can_read(uid) {
            return Err(MemoryError::PermissionDenied);
        }
        
        region.inc_ref();
        Ok(region.phys_addr())
    }
    
    /// Detach from shared memory (decrement ref, free if zero)
    pub fn detach(&mut self, id: ShmId) -> MemoryResult<bool> {
        let should_free = {
            let region = self.regions.get(&id)
                .ok_or(MemoryError::NotFound)?;
            region.dec_ref()
        };
        
        if should_free {
            self.free_region(id)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    
    /// Free a region and its physical memory
    fn free_region(&mut self, id: ShmId) -> MemoryResult<()> {
        if let Some(region) = self.regions.remove(&id) {
            // Remove from named map if applicable
            if let Some(name) = &region.name {
                self.named.remove(name);
            }
            
            // Free physical frames
            if region.frames.contiguous {
                // Free contiguous frames
                for i in 0..region.frames.frame_count {
                    let addr = PhysicalAddress::new(
                        region.frames.start_addr.value() + i * FRAME_SIZE
                    );
                    let frame = Frame::new(addr);
                    let _ = physical::deallocate_frame(frame);
                }
            } else if let Some(frames) = region.frames.frames {
                // Free scattered frames
                for frame in frames {
                    let _ = physical::deallocate_frame(frame);
                }
            }
            
            // Update statistics
            self.stats.total_freed.fetch_add(1, Ordering::Relaxed);
            self.stats.current_regions.fetch_sub(1, Ordering::Relaxed);
            
            log::debug!("Freed shared memory region {}: {} bytes", id.0, region.size);
        }
        
        Ok(())
    }
    
    /// Get pool statistics
    pub fn stats(&self) -> &PoolStats {
        &self.stats
    }
    
    /// List all regions (for debugging)
    pub fn list_regions(&self) -> impl Iterator<Item = &ShmRegion> {
        self.regions.values()
    }
}

/// Global pool instance
static GLOBAL_POOL: Mutex<SharedMemoryPool> = Mutex::new(SharedMemoryPool::new());

/// Initialize shared memory subsystem
pub fn init() {
    log::info!("Shared memory pool initialized");
}

/// Allocate shared memory (global API)
pub fn allocate(size: usize, perms: ShmPermissions, owner: usize) -> MemoryResult<ShmId> {
    GLOBAL_POOL.lock().allocate(size, perms, owner)
}

/// Create named shared memory (POSIX shm_open)
pub fn create_named(name: String, size: usize, perms: ShmPermissions, owner: usize) -> MemoryResult<ShmId> {
    GLOBAL_POOL.lock().create_named(name, size, perms, owner)
}

/// Open named shared memory
pub fn open_named(name: &str) -> MemoryResult<ShmId> {
    GLOBAL_POOL.lock().open_named(name)
}

/// Attach to shared memory
pub fn attach(id: ShmId) -> MemoryResult<PhysicalAddress> {
    GLOBAL_POOL.lock().attach(id)
}

/// Attach with permission check
pub fn attach_checked(id: ShmId, uid: u32, need_write: bool) -> MemoryResult<PhysicalAddress> {
    GLOBAL_POOL.lock().attach_checked(id, uid, need_write)
}

/// Detach from shared memory
pub fn detach(id: ShmId) -> MemoryResult<bool> {
    GLOBAL_POOL.lock().detach(id)
}

/// Get pool statistics
pub fn get_stats() -> (u64, u64, usize, u64) {
    let pool = GLOBAL_POOL.lock();
    let stats = pool.stats();
    (
        stats.total_allocated.load(Ordering::Relaxed),
        stats.total_freed.load(Ordering::Relaxed),
        stats.current_regions.load(Ordering::Relaxed),
        stats.total_bytes_allocated.load(Ordering::Relaxed),
    )
}
