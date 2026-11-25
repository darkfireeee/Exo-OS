//! Anonymous, shared-frame, and cross-address-space `mmap` operations.

use alloc::boxed::Box;
use core::sync::atomic::Ordering;

use crate::memory::core::{PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::UserAddressSpace;
use crate::memory::virt::vma::{VmaBacking, VmaDescriptor, VmaFlags};

use super::munmap_shootdown::do_munmap_range_in_as;
use super::{
    get_current_user_as, prot_to_page_flags, prot_to_vma_flags, record_exec_mapping_denied,
    rollback_anon_pages, rollback_shm_into_process, validate_prot, MmapError, PtAllocOnly,
    MAP_FIXED, MAP_FIXED_NOREPLACE, MAP_SHARED, PROT_EXEC,
};

pub(super) fn resolve_mmap_range(
    user_as: &UserAddressSpace,
    addr: u64,
    len_aligned: usize,
    flags: u32,
) -> Result<(VirtAddr, VirtAddr), MmapError> {
    let fixed = (flags & (MAP_FIXED | MAP_FIXED_NOREPLACE)) != 0;
    let base = if fixed {
        if addr == 0 || !addr.is_multiple_of(PAGE_SIZE as u64) {
            return Err(MmapError::InvalidAddress);
        }
        VirtAddr::new(addr)
    } else {
        let hint = (addr != 0).then_some(VirtAddr::new(addr));
        user_as
            .find_free_gap(len_aligned, hint)
            .ok_or(MmapError::OutOfVirtualMemory)?
    };

    let end_raw = base
        .as_u64()
        .checked_add(len_aligned as u64)
        .ok_or(MmapError::OutOfVirtualMemory)?;
    if base.as_u64() < PAGE_SIZE as u64 || end_raw > crate::memory::core::layout::USER_END.as_u64() {
        return Err(MmapError::InvalidAddress);
    }
    Ok((base, VirtAddr::new(end_raw)))
}

pub(super) fn insert_vma_replacing_fixed(
    user_as: &UserAddressSpace,
    start: VirtAddr,
    len_aligned: usize,
    flags: u32,
    vma_ptr: *mut VmaDescriptor,
) -> Result<(), MmapError> {
    let replace_fixed = (flags & MAP_FIXED != 0) && (flags & MAP_FIXED_NOREPLACE == 0);
    if !replace_fixed {
        if unsafe { user_as.insert_vma(vma_ptr) } {
            return Ok(());
        }
        let _ = unsafe { Box::from_raw(vma_ptr) };
        return Err(if flags & MAP_FIXED_NOREPLACE != 0 {
            MmapError::Exists
        } else {
            MmapError::InvalidAddress
        });
    }

    for _attempt in 0..4 {
        if let Err(err) = do_munmap_range_in_as(user_as, start.as_u64(), len_aligned) {
            let _ = unsafe { Box::from_raw(vma_ptr) };
            return Err(err);
        }
        if unsafe { user_as.insert_vma(vma_ptr) } {
            return Ok(());
        }
    }
    let _ = unsafe { Box::from_raw(vma_ptr) };
    Err(MmapError::InvalidAddress)
}

/// Maps a memory region in the current user address space.
pub fn do_mmap(
    addr: u64,
    len: usize,
    prot: u32,
    flags: u32,
    _fd: i32,
    _off: u64,
) -> Result<usize, MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };
    do_mmap_in_as(user_as, addr, len, prot, flags)
}

/// Explicit variant for Ring1 servers authorized to manage a target AS.
pub fn do_mmap_in_as(
    user_as: &UserAddressSpace,
    addr: u64,
    len: usize,
    prot: u32,
    flags: u32,
) -> Result<usize, MmapError> {
    if len == 0 {
        return Err(MmapError::InvalidLength);
    }
    validate_prot(prot)?;
    if (prot & PROT_EXEC) != 0 {
        record_exec_mapping_denied(1, addr);
        return Err(MmapError::PermissionDenied);
    }

    let len_aligned = len
        .checked_add(PAGE_SIZE - 1)
        .ok_or(MmapError::InvalidLength)?
        & !(PAGE_SIZE - 1);

    user_as.stats.mmap_calls.fetch_add(1, Ordering::Relaxed);

    let (start, end) = resolve_mmap_range(user_as, addr, len_aligned, flags)?;

    let is_shared = flags & MAP_SHARED != 0;
    let page_flags = if is_shared {
        prot_to_page_flags(prot) | PageFlags::PINNED
    } else {
        prot_to_page_flags(prot)
    };
    let vma_flags = prot_to_vma_flags(prot, flags) | VmaFlags::ANONYMOUS;
    let backing = if is_shared {
        VmaBacking::Shared
    } else {
        VmaBacking::Anonymous
    };

    let vma = Box::new(VmaDescriptor::new(start, end, vma_flags, page_flags, backing));
    let vma_ptr = Box::into_raw(vma);
    insert_vma_replacing_fixed(user_as, start, len_aligned, flags, vma_ptr)?;

    if is_shared {
        let pt_alloc = PtAllocOnly;
        let n_pages = len_aligned / PAGE_SIZE;
        for i in 0..n_pages {
            let Ok(frame) = crate::memory::physical::allocator::buddy::alloc_pages(
                0,
                crate::memory::AllocFlags::ZEROED,
            ) else {
                rollback_anon_pages(user_as, start, i);
                return Err(MmapError::AllocFailed);
            };
            let virt = VirtAddr::new(start.as_u64() + (i * PAGE_SIZE) as u64);
            if unsafe { user_as.map_page(virt, frame, page_flags, &pt_alloc) }.is_err() {
                let _ = crate::memory::physical::allocator::buddy::free_pages(frame, 0);
                rollback_anon_pages(user_as, start, i);
                return Err(MmapError::AllocFailed);
            }
        }
    }

    Ok(start.as_u64() as usize)
}

/// Maps a slice of shared physical frames (e.g. file-backed or memfd `MAP_SHARED`)
/// into the current user address space.
pub fn do_mmap_shared_frames(
    addr: u64,
    len: usize,
    prot: u32,
    flags: u32,
    frames: &[crate::memory::core::Frame],
) -> Result<usize, MmapError> {
    if len == 0 || frames.is_empty() {
        return Err(MmapError::InvalidLength);
    }
    validate_prot(prot)?;

    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

    let len_aligned = len
        .checked_add(PAGE_SIZE - 1)
        .ok_or(MmapError::InvalidLength)?
        & !(PAGE_SIZE - 1);
    let n_pages = len_aligned / PAGE_SIZE;
    if frames.len() < n_pages {
        return Err(MmapError::InvalidLength);
    }

    let (start, end) = resolve_mmap_range(user_as, addr, len_aligned, flags)?;

    let page_flags = prot_to_page_flags(prot) | PageFlags::PINNED;
    let vma_flags = prot_to_vma_flags(prot, flags) | VmaFlags::SHARED;

    let vma = Box::new(VmaDescriptor::new(
        start,
        end,
        vma_flags,
        page_flags,
        VmaBacking::Shared,
    ));
    let vma_ptr = Box::into_raw(vma);
    insert_vma_replacing_fixed(user_as, start, len_aligned, flags, vma_ptr)?;

    let pt_alloc = PtAllocOnly;
    for (i, &frame) in frames.iter().take(n_pages).enumerate() {
        let virt = VirtAddr::new(start.as_u64() + (i * PAGE_SIZE) as u64);
        if unsafe { user_as.map_page(virt, frame, page_flags, &pt_alloc) }.is_err() {
            rollback_shm_into_process(user_as, start, i);
            return Err(MmapError::AllocFailed);
        }
    }

    Ok(start.as_u64() as usize)
}

/// Shares an already-mapped shared physical range `[src_addr, src_addr + align_up(len))`
/// from `src_as` into `dst_as`, placing it at `dst_hint` (or choosing a free gap if `0`).
pub fn do_share_range_between_as(
    src_as: &UserAddressSpace,
    src_addr: u64,
    dst_as: &UserAddressSpace,
    dst_hint: u64,
    len: usize,
    prot: u32,
) -> Result<usize, MmapError> {
    if len == 0 {
        return Err(MmapError::InvalidLength);
    }
    validate_prot(prot)?;
    if src_addr < PAGE_SIZE as u64 || !src_addr.is_multiple_of(PAGE_SIZE as u64) {
        return Err(MmapError::InvalidAddress);
    }
    let len_aligned = len
        .checked_add(PAGE_SIZE - 1)
        .ok_or(MmapError::InvalidLength)?
        & !(PAGE_SIZE - 1);
    let src_end_raw = src_addr
        .checked_add(len_aligned as u64)
        .ok_or(MmapError::InvalidLength)?;
    let user_end = crate::memory::core::layout::USER_END.as_u64();
    if src_end_raw > user_end {
        return Err(MmapError::InvalidAddress);
    }

    if !src_as.verify_vma_range_covered(VirtAddr::new(src_addr), VirtAddr::new(src_end_raw)) {
        return Err(MmapError::NotMapped);
    }

    let dst_start = if dst_hint != 0 {
        if dst_hint < PAGE_SIZE as u64 || !dst_hint.is_multiple_of(PAGE_SIZE as u64) {
            return Err(MmapError::InvalidAddress);
        }
        VirtAddr::new(dst_hint)
    } else {
        dst_as
            .find_free_gap(len_aligned, None)
            .ok_or(MmapError::OutOfVirtualMemory)?
    };
    let dst_end_raw = dst_start
        .as_u64()
        .checked_add(len_aligned as u64)
        .ok_or(MmapError::OutOfVirtualMemory)?;
    if dst_start.as_u64() < PAGE_SIZE as u64 || dst_end_raw > user_end {
        return Err(MmapError::InvalidAddress);
    }
    let dst_end = VirtAddr::new(dst_end_raw);

    let page_flags = prot_to_page_flags(prot) | PageFlags::PINNED;
    let vma_flags = prot_to_vma_flags(prot, MAP_SHARED) | VmaFlags::SHARED;
    let vma = Box::new(VmaDescriptor::new(
        dst_start,
        dst_end,
        vma_flags,
        page_flags,
        VmaBacking::Shared,
    ));
    let vma_ptr = Box::into_raw(vma);
    if !unsafe { dst_as.insert_vma(vma_ptr) } {
        let _ = unsafe { Box::from_raw(vma_ptr) };
        return Err(MmapError::InvalidAddress);
    }

    let pt_alloc = PtAllocOnly;

    let same_as = core::ptr::eq(src_as, dst_as);
    let (_g1, _g2) = if same_as {
        (Some(src_as.lock_pt()), None)
    } else if src_as.pid <= dst_as.pid {
        (Some(src_as.lock_pt()), Some(dst_as.lock_pt()))
    } else {
        let d = dst_as.lock_pt();
        let s = src_as.lock_pt();
        (Some(s), Some(d))
    };

    let src_walker = crate::memory::virt::page_table::PageTableWalker::new(src_as.pml4_phys());
    let n_pages = len_aligned / PAGE_SIZE;
    for i in 0..n_pages {
        let src_va = VirtAddr::new(src_addr + (i * PAGE_SIZE) as u64);
        let dst_va = VirtAddr::new(dst_start.as_u64() + (i * PAGE_SIZE) as u64);
        let src_pte = crate::memory::virt::page_table::PageTableEntry::from_raw(
            src_walker.read_pte_raw(src_va),
        );
        if !src_pte.is_present() || !src_pte.is_user() || !src_pte.is_pinned() {
            rollback_shm_into_process(dst_as, dst_start, i);
            return Err(MmapError::PermissionDenied);
        }
        let Some(frame) = src_pte.frame() else {
            rollback_shm_into_process(dst_as, dst_start, i);
            return Err(MmapError::NotMapped);
        };
        if unsafe { dst_as.map_page(dst_va, frame, page_flags, &pt_alloc) }.is_err() {
            rollback_shm_into_process(dst_as, dst_start, i);
            return Err(MmapError::AllocFailed);
        }
    }

    Ok(dst_start.as_u64() as usize)
}
