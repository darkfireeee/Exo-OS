//! IPC SHM provider callback injection and `map_shm_into_process`.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::memory::core::{Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::UserAddressSpace;
use crate::memory::virt::vma::{VmaBacking, VmaDescriptor, VmaFlags};

use super::{rollback_shm_into_process, PtAllocOnly};

/// Errors specific to SHM mapping in an address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShmMapError {
    NoAddressSpace,
    OutOfVirtualMemory,
    AllocFailed,
    InvalidRegion,
    PermissionDenied,
}

/// Minimal SHM region metadata provided by the IPC layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShmRegionInfo {
    pub n_pages: usize,
    pub size_bytes: usize,
}

pub type ShmRegionInfoFn =
    fn(desc_idx: usize, writable: bool) -> Result<ShmRegionInfo, ShmMapError>;
pub type ShmPagePhysFn = fn(desc_idx: usize, page_idx: usize) -> Option<u64>;
pub type ShmReleaseRegionFn = fn(desc_idx: usize);
pub type ShmRegisterMappingFn = fn(
    desc_idx: usize,
    pid: u32,
    virt_base: u64,
    writable: bool,
    n_pages: usize,
) -> Result<usize, ShmMapError>;

/// IPC callbacks necessary to map an SHM region without importing IPC into `memory/`.
#[derive(Clone, Copy)]
pub struct ShmProviderFns {
    pub region_info: ShmRegionInfoFn,
    pub page_phys: ShmPagePhysFn,
    pub release_region: ShmReleaseRegionFn,
    pub register_mapping: ShmRegisterMappingFn,
}

static SHM_REGION_INFO_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SHM_PAGE_PHYS_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SHM_RELEASE_REGION_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SHM_REGISTER_MAPPING_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

pub fn register_shm_provider(provider: ShmProviderFns) {
    SHM_REGION_INFO_FN.store(provider.region_info as *mut (), Ordering::Release);
    SHM_PAGE_PHYS_FN.store(provider.page_phys as *mut (), Ordering::Release);
    SHM_RELEASE_REGION_FN.store(provider.release_region as *mut (), Ordering::Release);
    SHM_REGISTER_MAPPING_FN.store(provider.register_mapping as *mut (), Ordering::Release);
}

fn shm_provider() -> Option<ShmProviderFns> {
    let region_info = SHM_REGION_INFO_FN.load(Ordering::Acquire);
    let page_phys = SHM_PAGE_PHYS_FN.load(Ordering::Acquire);
    let release_region = SHM_RELEASE_REGION_FN.load(Ordering::Acquire);
    let register_mapping = SHM_REGISTER_MAPPING_FN.load(Ordering::Acquire);
    if region_info.is_null()
        || page_phys.is_null()
        || release_region.is_null()
        || register_mapping.is_null()
    {
        return None;
    }
    Some(ShmProviderFns {
        // SAFETY: registered exclusively via register_shm_provider with expected signatures.
        region_info: unsafe { core::mem::transmute::<*mut (), ShmRegionInfoFn>(region_info) },
        page_phys: unsafe { core::mem::transmute::<*mut (), ShmPagePhysFn>(page_phys) },
        release_region: unsafe {
            core::mem::transmute::<*mut (), ShmReleaseRegionFn>(release_region)
        },
        register_mapping: unsafe {
            core::mem::transmute::<*mut (), ShmRegisterMappingFn>(register_mapping)
        },
    })
}

/// Result of an SHM mapping into an address space.
pub struct ShmMapIntoResult {
    pub virt_base: u64,
    pub n_pages: usize,
    pub mapping_idx: usize,
}

/// Maps an existing SHM region into a process's address space.
pub fn map_shm_into_process(
    user_as: &UserAddressSpace,
    desc_idx: usize,
    pid: u32,
    hint_virt: u64,
    writable: bool,
) -> Result<ShmMapIntoResult, ShmMapError> {
    let provider = shm_provider().ok_or(ShmMapError::InvalidRegion)?;

    let info = (provider.region_info)(desc_idx, writable)?;
    let n_pages = info.n_pages;
    let size_bytes = info.size_bytes;

    if n_pages == 0 {
        (provider.release_region)(desc_idx);
        return Err(ShmMapError::InvalidRegion);
    }

    let size_aligned = (size_bytes + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let hint = if hint_virt != 0 {
        Some(VirtAddr::new(hint_virt))
    } else {
        None
    };
    let virt_base = user_as.find_free_gap(size_aligned, hint).ok_or_else(|| {
        (provider.release_region)(desc_idx);
        ShmMapError::OutOfVirtualMemory
    })?;

    let virt_end = VirtAddr::new(virt_base.as_u64() + size_aligned as u64);

    let vma_flags = if writable {
        VmaFlags::READ | VmaFlags::WRITE | VmaFlags::SHARED
    } else {
        VmaFlags::READ | VmaFlags::SHARED
    };
    let page_flags = if writable {
        PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE
    } else {
        PageFlags::PRESENT | PageFlags::USER | PageFlags::NO_EXECUTE
    };

    let vma = Box::new(VmaDescriptor::new(
        virt_base,
        virt_end,
        vma_flags,
        page_flags,
        VmaBacking::Shared,
    ));
    let vma_ptr = Box::into_raw(vma);
    let inserted = unsafe { user_as.insert_vma(vma_ptr) };
    if !inserted {
        let _ = unsafe { Box::from_raw(vma_ptr) };
        (provider.release_region)(desc_idx);
        return Err(ShmMapError::OutOfVirtualMemory);
    }

    let pt_alloc = PtAllocOnly;
    for i in 0..n_pages {
        let Some(phys) = (provider.page_phys)(desc_idx, i) else {
            rollback_shm_into_process(user_as, virt_base, i);
            (provider.release_region)(desc_idx);
            return Err(ShmMapError::InvalidRegion);
        };
        let frame = Frame::containing(PhysAddr::new(phys));
        let virt = VirtAddr::new(virt_base.as_u64().saturating_add((i * PAGE_SIZE) as u64));
        if unsafe { user_as.map_page(virt, frame, page_flags, &pt_alloc) }.is_err() {
            rollback_shm_into_process(user_as, virt_base, i);
            (provider.release_region)(desc_idx);
            return Err(ShmMapError::AllocFailed);
        }
    }

    let mapping_idx =
        (provider.register_mapping)(desc_idx, pid, virt_base.as_u64(), writable, n_pages)
            .inspect_err(|_| {
                rollback_shm_into_process(user_as, virt_base, n_pages);
                (provider.release_region)(desc_idx);
            })?;

    Ok(ShmMapIntoResult {
        virt_base: virt_base.as_u64(),
        n_pages,
        mapping_idx,
    })
}
