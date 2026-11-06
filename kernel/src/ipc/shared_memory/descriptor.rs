// ipc/shared_memory/descriptor.rs — SHM region descriptor for Exo-OS
//
// ShmDescriptor is the main structure describing an IPC shared memory
// region: identifier, size, owner, permissions, list of associated
// physical pages.
//
// Constraints:
//   - Maximum size: SHM_MAX_PAGES * PAGE_SIZE (256 × 4 KiB = 1 MiB)
//   - No dynamic allocation — inline page list (MAX_SHM_PAGES_PER_DESC)
//   - All pages must have the NO_COW flag

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::{IpcError, ProcessId};
use crate::ipc::shared_memory::page::{PhysAddr, PAGE_SIZE};
use crate::ipc::shared_memory::pool::{shm_page_alloc, shm_page_free, shm_page_phys};

// ---------------------------------------------------------------------------
// SHM Identifiers
// ---------------------------------------------------------------------------

/// Unique identifier of an SHM region
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ShmId(u32);

impl ShmId {
    pub const INVALID: Self = Self(0);

    pub fn new(v: u32) -> Option<Self> {
        if v != 0 {
            Some(Self(v))
        } else {
            None
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

/// Counter to generate unique ShmIds
static SHM_ID_COUNTER: AtomicU32 = AtomicU32::new(1);

pub fn alloc_shm_id() -> ShmId {
    let v = SHM_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    ShmId(v)
}

// ---------------------------------------------------------------------------
// SHM Permissions
// ---------------------------------------------------------------------------

/// Access permissions for an SHM region
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShmPermissions(pub u8);

impl ShmPermissions {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const READ_WRITE: Self = Self(Self::READ.0 | Self::WRITE.0);

    pub fn can_read(self) -> bool {
        (self.0 & Self::READ.0) != 0
    }

    pub fn can_write(self) -> bool {
        (self.0 & Self::WRITE.0) != 0
    }
}

// ---------------------------------------------------------------------------
// Maximum number of pages per descriptor
// ---------------------------------------------------------------------------

/// Maximum number of physical pages per SHM region (inline limit)
pub const MAX_SHM_PAGES_PER_DESC: usize = 64; // 64 × 4 KiB = 256 KiB max

// ---------------------------------------------------------------------------
// SHM region state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ShmState {
    /// Uninitialized
    Uninitialized = 0,
    /// Active and accessible
    Active = 1,
    /// Closing in progress (no new mappings allowed)
    Closing = 2,
    /// Destroyed
    Destroyed = 3,
}

impl ShmState {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Active,
            2 => Self::Closing,
            3 => Self::Destroyed,
            _ => Self::Uninitialized,
        }
    }
}

// ---------------------------------------------------------------------------
// ShmDescriptor — main structure
// ---------------------------------------------------------------------------

/// Descriptor of an IPC shared memory region.
///
/// Contains all metadata and the list of physical pages
/// composing the region.
#[repr(C, align(64))]
pub struct ShmDescriptor {
    /// Unique identifier of the region
    pub id: ShmId,
    /// Owner process
    pub owner: ProcessId,
    /// Current state
    pub state: AtomicU32,
    /// Permissions declared at creation
    pub permissions: u8,
    _perm_pad: [u8; 3],
    /// Number of allocated pages
    pub page_count: AtomicU32,
    /// Total size in bytes (page_count * PAGE_SIZE)
    pub size_bytes: u64,
    /// Active mapping count (references by processes)
    pub mapping_count: AtomicU32,
    /// Creation timestamp (ns since boot)
    pub created_at: AtomicU64,
    /// Last access timestamp
    pub last_access: AtomicU64,
    /// Total access count
    pub access_count: AtomicU64,
    /// Page indices in the SHM pool (MAX_SHM_PAGES_PER_DESC max)
    pub pages: [AtomicU32; MAX_SHM_PAGES_PER_DESC],
    _pad: [u8; 8],
}

// SAFETY: ShmId(u32) + ProcessId(u32) are Copy; AtomicU32/AtomicU64 are Sync
unsafe impl Sync for ShmDescriptor {}
unsafe impl Send for ShmDescriptor {}

impl ShmDescriptor {
    /// Creates an uninitialized descriptor
    pub const fn new_uninit() -> Self {
        const INIT_ATOMIC: AtomicU32 = AtomicU32::new(u32::MAX);
        Self {
            id: ShmId::INVALID,
            owner: ProcessId(0),
            state: AtomicU32::new(ShmState::Uninitialized as u32),
            permissions: 0,
            _perm_pad: [0u8; 3],
            page_count: AtomicU32::new(0),
            size_bytes: 0,
            mapping_count: AtomicU32::new(0),
            created_at: AtomicU64::new(0),
            last_access: AtomicU64::new(0),
            access_count: AtomicU64::new(0),
            pages: [INIT_ATOMIC; MAX_SHM_PAGES_PER_DESC],
            _pad: [0u8; 8],
        }
    }

    pub fn state(&self) -> ShmState {
        ShmState::from_u32(self.state.load(Ordering::Acquire))
    }

    pub fn is_active(&self) -> bool {
        self.state() == ShmState::Active
    }

    pub fn page_count(&self) -> usize {
        self.page_count.load(Ordering::Acquire) as usize
    }

    /// Returns the physical address of page `i` of the region.
    pub fn page_phys(&self, i: usize) -> Option<PhysAddr> {
        if i >= self.page_count() {
            return None;
        }
        let idx = self.pages[i].load(Ordering::Relaxed) as usize;
        shm_page_phys(idx)
    }

    /// Returns the pool index of page `i`.
    pub fn page_pool_idx(&self, i: usize) -> Option<usize> {
        if i >= self.page_count() {
            return None;
        }
        let idx = self.pages[i].load(Ordering::Relaxed);
        if idx == u32::MAX {
            None
        } else {
            Some(idx as usize)
        }
    }

    /// Increments the active mapping counter.
    pub fn add_mapping(&self) {
        self.mapping_count.fetch_add(1, Ordering::Relaxed);
        self.access_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrements the active mapping counter.
    /// Returns `true` if there are no more active mappings.
    pub fn remove_mapping(&self) -> bool {
        let mut current = self.mapping_count.load(Ordering::Acquire);
        loop {
            if current == 0 {
                return true;
            }
            match self.mapping_count.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(prev) => return prev == 1,
                Err(next) => current = next,
            }
        }
    }

    pub fn mapping_count(&self) -> u32 {
        self.mapping_count.load(Ordering::Relaxed)
    }

    /// Stop new mappings while an existing region is being revoked/destroyed.
    pub fn begin_close(&self) -> bool {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            match ShmState::from_u32(state) {
                ShmState::Active => match self.state.compare_exchange_weak(
                    state,
                    ShmState::Closing as u32,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return true,
                    Err(next) => state = next,
                },
                ShmState::Closing => return true,
                ShmState::Uninitialized | ShmState::Destroyed => return false,
            }
        }
    }

    /// Frees all allocated pages and transitions to the Destroyed state.
    pub fn destroy(&self) {
        self.state
            .store(ShmState::Destroyed as u32, Ordering::Release);
        let n = self.page_count();
        for i in 0..n {
            let pool_idx = self.pages[i].load(Ordering::Relaxed) as usize;
            if pool_idx < crate::ipc::core::constants::SHM_POOL_PAGES {
                shm_page_free(pool_idx);
                self.pages[i].store(u32::MAX, Ordering::Relaxed);
            }
        }
        self.page_count.store(0, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Descriptor directory allocator
// ---------------------------------------------------------------------------

/// Maximum number of SHM regions that can coexist simultaneously
pub const MAX_SHM_REGIONS: usize = 1024;

/// Entry in the SHM descriptor directory
pub struct ShmDescEntry {
    pub desc: MaybeUninit<ShmDescriptor>,
    pub used: bool,
}

impl ShmDescEntry {
    pub const fn new() -> Self {
        Self {
            desc: MaybeUninit::uninit(),
            used: false,
        }
    }
}

/// Global SHM descriptor directory (protected by SpinLock)
pub struct ShmDescDirectory {
    entries: [ShmDescEntry; MAX_SHM_REGIONS],
    count: usize,
}

// SAFETY: access protected by SpinLock in the global table
unsafe impl Send for ShmDescDirectory {}

impl ShmDescDirectory {
    pub const fn new() -> Self {
        const INIT: ShmDescEntry = ShmDescEntry::new();
        Self {
            entries: [INIT; MAX_SHM_REGIONS],
            count: 0,
        }
    }

    /// Allocates a new descriptor and returns its index.
    pub fn alloc(
        &mut self,
        owner: ProcessId,
        perms: ShmPermissions,
        n_pages: usize,
    ) -> Result<usize, IpcError> {
        if n_pages == 0 || n_pages > MAX_SHM_PAGES_PER_DESC {
            return Err(IpcError::InvalidArgument);
        }

        // Look for a free slot
        for i in 0..MAX_SHM_REGIONS {
            if !self.entries[i].used {
                let mut desc = ShmDescriptor::new_uninit();
                desc.id = alloc_shm_id();
                desc.owner = owner;
                desc.permissions = perms.0;

                // Allocate pages from the pool
                let mut allocated = 0usize;
                for j in 0..n_pages {
                    match shm_page_alloc() {
                        Some(page_idx) => {
                            desc.pages[j].store(page_idx as u32, Ordering::Relaxed);
                            allocated += 1;
                        }
                        None => {
                            // Roll back allocations already made
                            for k in 0..allocated {
                                let idx = desc.pages[k].load(Ordering::Relaxed) as usize;
                                shm_page_free(idx);
                            }
                            return Err(IpcError::OutOfResources);
                        }
                    }
                }

                desc.page_count.store(n_pages as u32, Ordering::Relaxed);
                desc.size_bytes = (n_pages * PAGE_SIZE) as u64;
                desc.state.store(ShmState::Active as u32, Ordering::Release);

                self.entries[i].desc.write(desc);
                self.entries[i].used = true;
                self.count += 1;
                return Ok(i);
            }
        }
        Err(IpcError::OutOfResources)
    }

    pub fn free(&mut self, idx: usize) -> bool {
        if idx < MAX_SHM_REGIONS && self.entries[idx].used {
            // SAFETY: used is true -> desc is initialized
            let desc = unsafe { self.entries[idx].desc.assume_init_ref() };
            if desc.mapping_count() != 0 {
                return false;
            }
            desc.destroy();
            // SAFETY: desc initialized (used is true); used -> false immediately after, prevents double-drop.
            unsafe { self.entries[idx].desc.assume_init_drop() };
            self.entries[idx].used = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    pub unsafe fn get(&self, idx: usize) -> Option<&ShmDescriptor> {
        if idx < MAX_SHM_REGIONS && self.entries[idx].used {
            Some(self.entries[idx].desc.assume_init_ref())
        } else {
            None
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

use crate::scheduler::sync::spinlock::SpinLock;

pub static SHM_DESC_DIR: SpinLock<ShmDescDirectory> = SpinLock::new(ShmDescDirectory::new());

// ---------------------------------------------------------------------------
// High-level public API
// ---------------------------------------------------------------------------

/// Creates a new SHM region of `n_pages` pages.
/// Returns the index in the directory.
pub fn shm_create(
    owner: ProcessId,
    perms: ShmPermissions,
    n_pages: usize,
) -> Result<usize, IpcError> {
    let mut dir = SHM_DESC_DIR.lock();
    dir.alloc(owner, perms, n_pages)
}

/// Returns the SHM ID associated with index `idx`.
pub fn shm_get_id(idx: usize) -> Option<ShmId> {
    let dir = SHM_DESC_DIR.lock();
    let desc = unsafe { dir.get(idx) }?;
    Some(desc.id)
}

/// Returns the size in bytes of region `idx`.
pub fn shm_get_size(idx: usize) -> Option<u64> {
    let dir = SHM_DESC_DIR.lock();
    let desc = unsafe { dir.get(idx) }?;
    Some(desc.size_bytes)
}

/// Destroys SHM region `idx` (frees all its pages).
pub fn shm_destroy(idx: usize) -> Result<(), IpcError> {
    {
        let dir = SHM_DESC_DIR.lock();
        let desc = unsafe { dir.get(idx) }.ok_or(IpcError::InvalidHandle)?;
        if !desc.begin_close() {
            return Err(IpcError::InvalidHandle);
        }
    }

    crate::ipc::shared_memory::mapping::shm_unmap_all_for_desc(idx);

    let mut dir = SHM_DESC_DIR.lock();
    if !dir.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Returns the number of active SHM regions.
pub fn shm_region_count() -> usize {
    SHM_DESC_DIR.lock().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_mapping_on_zero_does_not_underflow() {
        let desc = ShmDescriptor::new_uninit();

        assert!(desc.remove_mapping());
        assert_eq!(desc.mapping_count(), 0);
    }

    #[test]
    fn add_remove_mapping_round_trip() {
        let desc = ShmDescriptor::new_uninit();

        desc.add_mapping();
        assert_eq!(desc.mapping_count(), 1);
        assert!(desc.remove_mapping());
        assert_eq!(desc.mapping_count(), 0);
    }
}
