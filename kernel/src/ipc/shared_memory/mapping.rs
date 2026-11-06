// ipc/shared_memory/mapping.rs — Mapping SHM regions into address spaces
//
// This module manages the association between an SHM region (ShmDescriptor) and a
// process virtual address space.
//
// In Exo-OS, the memory manager manages page tables. This module is an
// IPC layer that:
//   1. Records active mappings (which region -> which process)
//   2. Validates permissions on each access
//   3. Delegates the actual mapping to the memory manager (opaque interface)
//   4. Applies the NO_COW flag to all mapped pages
//
// Mapping hooks (MapPageFn / UnmapPageFn) are connected at boot via
// `ipc::ipc_install_vmm_hooks()`. Without installed hooks, shm_map() fails in
// production; simulated virt=phys mode is restricted to the `dev_no_vmm` feature.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::ipc::core::types::{IpcError, ProcessId};
use crate::ipc::shared_memory::descriptor::{ShmPermissions, MAX_SHM_PAGES_PER_DESC, SHM_DESC_DIR};
use crate::ipc::shared_memory::page::{PageFlags, PhysAddr, PAGE_SIZE};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Virtual address
// ---------------------------------------------------------------------------

/// Virtual address in a process's space
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    pub const NULL: Self = Self(0);

    pub fn is_null(self) -> bool {
        self.0 == 0
    }

    pub fn is_page_aligned(self) -> bool {
        (self.0 & (PAGE_SIZE as u64 - 1)) == 0
    }

    pub fn is_canonical(self) -> bool {
        let sign = (self.0 >> 47) & 1;
        let upper = self.0 >> 48;
        (sign == 0 && upper == 0) || (sign == 1 && upper == 0xffff)
    }
}

// ---------------------------------------------------------------------------
// Memory manager hook (future interface)
// ---------------------------------------------------------------------------

/// Function pointer type to map a physical page into virtual memory.
/// To connect with memory::virtual::map_page() during integration.
///
/// Arguments: (phys_addr, virt_addr, flags, process_id)
/// Return:    0 = ok, non-zero = error
pub type MapPageFn = unsafe fn(phys: u64, virt: u64, flags: u32, pid: u32) -> i32;

/// Optional page mapping hook (None = simulated mapping)
static MAP_PAGE_HOOK: SpinLock<Option<MapPageFn>> = SpinLock::new(None);

/// Optional page unmapping hook
pub type UnmapPageFn = unsafe fn(virt: u64, pid: u32) -> i32;
static UNMAP_PAGE_HOOK: SpinLock<Option<UnmapPageFn>> = SpinLock::new(None);
static VMM_HOOKS_INSTALLED: AtomicBool = AtomicBool::new(false);

#[inline]
fn refresh_vmm_hooks_ready() {
    let ready = MAP_PAGE_HOOK.lock().is_some() && UNMAP_PAGE_HOOK.lock().is_some();
    VMM_HOOKS_INSTALLED.store(ready, Ordering::Release);
}

/// Registers the mapping hook provided by the memory manager.
pub fn register_map_hook(f: MapPageFn) {
    *MAP_PAGE_HOOK.lock() = Some(f);
    refresh_vmm_hooks_ready();
}

/// Registers the unmapping hook provided by the memory manager.
pub fn register_unmap_hook(f: UnmapPageFn) {
    *UNMAP_PAGE_HOOK.lock() = Some(f);
    refresh_vmm_hooks_ready();
}

/// Returns true if real VMM hooks are installed.
#[inline]
pub fn vmm_hooks_ready() -> bool {
    VMM_HOOKS_INSTALLED.load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------
// Mapping registration
// ---------------------------------------------------------------------------

/// Maximum number of simultaneously active mappings
pub const MAX_SHM_MAPPINGS: usize = 2048;

const SHM_MAPPING_FREE: u32 = 0;
const SHM_MAPPING_RESERVED: u32 = 1;
const SHM_MAPPING_ACTIVE: u32 = 2;

/// Represents an active mapping: (region, process, virtual address)
#[repr(C, align(64))]
pub struct ShmMapping {
    /// Index of the SHM region in SHM_DESC_DIR
    pub desc_idx: AtomicU32,
    /// Identifier of the process requesting the mapping
    pub process_id: AtomicU32,
    /// Virtual base address of the mapping in the process space
    pub virt_base: AtomicU64,
    /// Effective permissions of the mapping (intersection of creator + requester)
    pub permissions: AtomicU32,
    /// Number of mapped pages
    pub mapped_pages: AtomicU32,
    /// Creation timestamp of the mapping
    pub created_at: AtomicU64,
    /// Active / freed mapping
    pub active: AtomicU32,
    _pad: [u8; 12],
}

// SAFETY: all fields are atomic
unsafe impl Sync for ShmMapping {}
unsafe impl Send for ShmMapping {}

impl ShmMapping {
    pub const fn new_uninit() -> Self {
        Self {
            desc_idx: AtomicU32::new(u32::MAX),
            process_id: AtomicU32::new(0),
            virt_base: AtomicU64::new(0),
            permissions: AtomicU32::new(0),
            mapped_pages: AtomicU32::new(0),
            created_at: AtomicU64::new(0),
            active: AtomicU32::new(0),
            _pad: [0u8; 12],
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire) == SHM_MAPPING_ACTIVE
    }

    pub(crate) fn mark_active(&self) {
        self.active.store(SHM_MAPPING_ACTIVE, Ordering::Release);
    }

    pub fn process_id(&self) -> ProcessId {
        ProcessId(self.process_id.load(Ordering::Relaxed))
    }

    pub fn desc_idx(&self) -> usize {
        self.desc_idx.load(Ordering::Relaxed) as usize
    }

    pub fn virt_base(&self) -> VirtAddr {
        VirtAddr(self.virt_base.load(Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Active mappings table
// ---------------------------------------------------------------------------

/// Global table of active SHM mappings
pub(crate) struct ShmMappingTable {
    entries: [ShmMapping; MAX_SHM_MAPPINGS],
    count: usize,
}

// SAFETY: access protected by SpinLock
unsafe impl Send for ShmMappingTable {}

impl ShmMappingTable {
    const fn new() -> Self {
        const INIT: ShmMapping = ShmMapping::new_uninit();
        Self {
            entries: [INIT; MAX_SHM_MAPPINGS],
            count: 0,
        }
    }

    pub(crate) fn alloc(&mut self) -> Option<usize> {
        for i in 0..MAX_SHM_MAPPINGS {
            if self.entries[i].active.load(Ordering::Acquire) == SHM_MAPPING_FREE {
                self.entries[i]
                    .active
                    .store(SHM_MAPPING_RESERVED, Ordering::Release);
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    pub(crate) fn free(&mut self, idx: usize) -> bool {
        if idx < MAX_SHM_MAPPINGS
            && self.entries[idx].active.load(Ordering::Acquire) != SHM_MAPPING_FREE
        {
            let entry = &self.entries[idx];
            entry.desc_idx.store(u32::MAX, Ordering::Relaxed);
            entry.process_id.store(0, Ordering::Relaxed);
            entry.virt_base.store(0, Ordering::Relaxed);
            entry.permissions.store(0, Ordering::Relaxed);
            entry.mapped_pages.store(0, Ordering::Relaxed);
            entry.created_at.store(0, Ordering::Relaxed);
            entry.active.store(SHM_MAPPING_FREE, Ordering::Release);
            self.count = self.count.saturating_sub(1);
            true
        } else {
            false
        }
    }

    pub(crate) fn entry(&self, idx: usize) -> Option<&ShmMapping> {
        self.entries.get(idx)
    }

    pub(crate) fn active_mapping_for_desc(&self, desc_idx: usize) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.is_active() && entry.desc_idx() == desc_idx)
            .map(|(idx, _)| idx)
    }
}

pub(crate) static SHM_MAPPING_TABLE: SpinLock<ShmMappingTable> =
    SpinLock::new(ShmMappingTable::new());

// ---------------------------------------------------------------------------
// Mapping operation
// ---------------------------------------------------------------------------

/// Result of an SHM mapping operation
#[derive(Debug, Clone, Copy)]
pub struct ShmMapResult {
    /// Mapping index in the table
    pub mapping_idx: usize,
    /// Virtual base address
    pub virt_base: VirtAddr,
    /// Mapped size in bytes
    pub mapped_size: u64,
}

/// Maps SHM region `desc_idx` into the address space of process `pid`.
///
/// `hint_virt`: desired virtual address (0 = left to memory manager).
/// Permissions: intersection of region rights and `requested_perms`.
///
/// # Errors
/// - `IpcError::InvalidHandle` — nonexistent or destroyed region
/// - `IpcError::PermissionDenied` — insufficient permissions
/// - `IpcError::OutOfResources` — mapping table full
/// - `IpcError::MappingFailed` — memory manager error
pub fn shm_map(
    desc_idx: usize,
    pid: ProcessId,
    hint_virt: VirtAddr,
    requested_perms: ShmPermissions,
) -> Result<ShmMapResult, IpcError> {
    #[cfg(not(feature = "dev_no_vmm"))]
    if !vmm_hooks_ready() {
        return Err(IpcError::MappingFailed);
    }

    // Check that the region exists, is active, and authorizes requested permissions
    let (n_pages, region_perms, size_bytes) = {
        let dir = SHM_DESC_DIR.lock();
        let desc = unsafe { dir.get(desc_idx) }.ok_or(IpcError::InvalidHandle)?;
        if !desc.is_active() {
            return Err(IpcError::InvalidHandle);
        }
        // Calculate effective permissions (intersection)
        let rp = ShmPermissions(requested_perms.0 & desc.permissions);
        if requested_perms.can_write() && !rp.can_write() {
            return Err(IpcError::PermissionDenied);
        }
        let n = desc.page_count();
        let sz = desc.size_bytes;
        desc.add_mapping();
        (n, rp, sz)
    };

    // Allocate a mapping slot
    let mapping_idx = {
        let mut tbl = SHM_MAPPING_TABLE.lock();
        match tbl.alloc() {
            Some(idx) => idx,
            None => {
                let dir = SHM_DESC_DIR.lock();
                if let Some(desc) = unsafe { dir.get(desc_idx) } {
                    desc.remove_mapping();
                }
                return Err(IpcError::OutOfResources);
            }
        }
    };

    // Calculate or use virtual address
    // If hint_virt == NULL, we use a dummy virtual address based on
    // the physical address of the first page (the real memory manager would place
    // it correctly in the process's address space).
    let virt_base = if hint_virt.is_null() {
        let phys = {
            let dir = SHM_DESC_DIR.lock();
            // SAFETY: desc_idx allocated by alloc() in SHM_DESC_DIR; lock held during access.
            unsafe { dir.get(desc_idx) }
                .and_then(|d| d.page_phys(0))
                .unwrap_or(PhysAddr::NULL)
        };
        // Virtual address = physical address in stub implementation
        // (will be replaced by memory::virtual::find_vma() during integration)
        VirtAddr(phys.0)
    } else {
        if !hint_virt.is_page_aligned() || !hint_virt.is_canonical() {
            let dir = SHM_DESC_DIR.lock();
            if let Some(desc) = unsafe { dir.get(desc_idx) } {
                desc.remove_mapping();
            }
            SHM_MAPPING_TABLE.lock().free(mapping_idx);
            return Err(IpcError::InvalidArgument);
        }
        hint_virt
    };

    // Extract physical addresses under lock, then release SHM_DESC_DIR
    // before invoking VMM hooks to prevent lock inversion (T8-025).
    let mut phys_addrs = [PhysAddr::NULL; MAX_SHM_PAGES_PER_DESC];
    {
        let dir = SHM_DESC_DIR.lock();
        if let Some(desc) = unsafe { dir.get(desc_idx) } {
            for i in 0..n_pages.min(MAX_SHM_PAGES_PER_DESC) {
                if let Some(phys) = desc.page_phys(i) {
                    phys_addrs[i] = phys;
                }
            }
        }
    }

    // Call mapping hook for each page
    {
        let map_hook = *MAP_PAGE_HOOK.lock();
        if let Some(map_fn) = map_hook {
            let mut page_flags = PageFlags::SHM_DEFAULT;
            // T8-001 FIX: remove WRITE if region permissions are read-only
            if !region_perms.can_write() {
                page_flags.remove(PageFlags::WRITE);
            }
            for i in 0..n_pages.min(MAX_SHM_PAGES_PER_DESC) {
                let phys = phys_addrs[i];
                if !phys.is_null() {
                    let virt = virt_base.0 + (i * PAGE_SIZE) as u64;
                    let result = unsafe { map_fn(phys.0, virt, page_flags.0, pid.0) };
                    if result != 0 {
                        // Roll back previous mappings
                        if let Some(unmap_fn) = *UNMAP_PAGE_HOOK.lock() {
                            for j in 0..i {
                                let v = virt_base.0 + (j * PAGE_SIZE) as u64;
                                // SAFETY: unmap_fn is a validated hook; v is in the mapped region (j < i < n_pages).
                                unsafe { unmap_fn(v, pid.0) };
                            }
                        }
                        // Release mapping count
                        let dir2 = SHM_DESC_DIR.lock();
                        if let Some(d) = unsafe { dir2.get(desc_idx) } {
                            d.remove_mapping();
                        }
                        SHM_MAPPING_TABLE.lock().free(mapping_idx);
                        return Err(IpcError::MappingFailed);
                    }
                }
            }
        }
        // If no hook -> simulated mapping (dev/test mode)
    }

    // Register mapping in table
    {
        let tbl = SHM_MAPPING_TABLE.lock();
        let m = &tbl.entries[mapping_idx];
        m.desc_idx.store(desc_idx as u32, Ordering::Relaxed);
        m.process_id.store(pid.0, Ordering::Relaxed);
        m.virt_base.store(virt_base.0, Ordering::Relaxed);
        m.permissions
            .store(region_perms.0 as u32, Ordering::Relaxed);
        m.mapped_pages.store(n_pages as u32, Ordering::Relaxed);
        m.mark_active();
    }

    Ok(ShmMapResult {
        mapping_idx,
        virt_base,
        mapped_size: size_bytes,
    })
}

/// Unmaps the SHM region identified by `mapping_idx`.
pub fn shm_unmap(mapping_idx: usize) -> Result<(), IpcError> {
    shm_unmap_for_process(mapping_idx, None)
}

/// Unmaps the SHM region identified by `mapping_idx` for the specified process (if specified).
pub fn shm_unmap_for_process(mapping_idx: usize, expected_pid: Option<ProcessId>) -> Result<(), IpcError> {
    let (desc_idx, pid, virt_base, n_pages) = {
        let mut tbl = SHM_MAPPING_TABLE.lock();
        if mapping_idx >= MAX_SHM_MAPPINGS || !tbl.entries[mapping_idx].is_active() {
            return Err(IpcError::InvalidHandle);
        }
        let m = &tbl.entries[mapping_idx];
        let actual_pid = ProcessId(m.process_id.load(Ordering::Relaxed));
        if let Some(exp) = expected_pid {
            if exp != actual_pid {
                return Err(IpcError::PermissionDenied);
            }
        }
        let desc = m.desc_idx() as usize;
        let virt = VirtAddr(m.virt_base.load(Ordering::Relaxed));
        let pages = m.mapped_pages.load(Ordering::Relaxed) as usize;
        // Atomically free the mapping slot under table lock
        // before calling VMM hooks to ensure no other thread
        // can execute a concurrent double unmap (T8-010).
        tbl.free(mapping_idx);
        (desc, actual_pid, virt, pages)
    };

    // Call unmapping hook
    if let Some(unmap_fn) = *UNMAP_PAGE_HOOK.lock() {
        for i in 0..n_pages {
            let virt = virt_base.0 + (i * PAGE_SIZE) as u64;
            // SAFETY: virt + pid come from a valid mapping
            unsafe { unmap_fn(virt, pid.0) };
        }
    }

    // Decrement mapping counter on the region
    let dir = SHM_DESC_DIR.lock();
    if let Some(desc) = unsafe { dir.get(desc_idx) } {
        desc.remove_mapping();
    }

    Ok(())
}

/// Revoke every active mapping of one descriptor before the descriptor pages
/// are returned to the SHM pool.
pub fn shm_unmap_all_for_desc(desc_idx: usize) -> usize {
    let mut unmapped = 0usize;

    loop {
        let mapping_idx = SHM_MAPPING_TABLE.lock().active_mapping_for_desc(desc_idx);
        let Some(mapping_idx) = mapping_idx else {
            break;
        };
        if shm_unmap(mapping_idx).is_err() {
            break;
        }
        unmapped += 1;
    }

    unmapped
}

/// Returns the number of active mappings.
pub fn shm_mapping_count() -> usize {
    SHM_MAPPING_TABLE.lock().count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_table_reserves_slots_before_activation() {
        let mut table = ShmMappingTable::new();

        let first = table.alloc().expect("first reservation");
        let second = table.alloc().expect("second reservation");

        assert_ne!(first, second);
        assert_eq!(table.count, 2);
        assert!(!table.entries[first].is_active());
        assert!(!table.entries[second].is_active());

        table.entries[first].mark_active();
        assert!(table.entries[first].is_active());
        assert!(!table.entries[second].is_active());
    }

    #[test]
    fn mapping_table_free_releases_reserved_slot_and_count() {
        let mut table = ShmMappingTable::new();
        let idx = table.alloc().expect("reservation");

        assert_eq!(table.count, 1);
        assert!(table.free(idx));
        assert_eq!(table.count, 0);

        let reused = table.alloc().expect("reused slot");
        assert_eq!(reused, idx);
    }
}
