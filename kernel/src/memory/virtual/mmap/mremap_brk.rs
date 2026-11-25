//! Zero-copy `mremap` PTE relocation and per-process `brk` heap management.

use alloc::boxed::Box;
use core::sync::atomic::Ordering;

use crate::memory::core::{PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::{UserAddressSpace, USER_MMAP_BASE};
use crate::memory::virt::vma::{VmaBacking, VmaDescriptor, VmaFlags};

use super::{free_removed_vma, get_current_user_as, MmapError, MremapZeroCopy, PtAllocOnly};

/// Moves a complete VMA by moving PTEs, without copying data.
pub fn do_mremap_zero_copy(
    old_addr: u64,
    old_len: usize,
    new_len: usize,
    flags: u64,
    new_addr: u64,
) -> Result<MremapZeroCopy, MmapError> {
    const MREMAP_MAYMOVE: u64 = 0x1;
    const MREMAP_FIXED: u64 = 0x2;
    const MREMAP_DONTUNMAP: u64 = 0x4;

    if old_addr < PAGE_SIZE as u64 || !old_addr.is_multiple_of(PAGE_SIZE as u64) {
        return Err(MmapError::InvalidAddress);
    }
    if old_len == 0 || new_len == 0 {
        return Err(MmapError::InvalidLength);
    }
    if flags & MREMAP_DONTUNMAP != 0 || flags & MREMAP_MAYMOVE == 0 {
        return Ok(MremapZeroCopy::Unsupported);
    }
    if flags & MREMAP_FIXED != 0
        && (new_addr < PAGE_SIZE as u64 || !new_addr.is_multiple_of(PAGE_SIZE as u64))
    {
        return Err(MmapError::InvalidAddress);
    }

    let old_len_aligned = old_len
        .checked_add(PAGE_SIZE - 1)
        .ok_or(MmapError::InvalidLength)?
        & !(PAGE_SIZE - 1);
    let new_len_aligned = new_len
        .checked_add(PAGE_SIZE - 1)
        .ok_or(MmapError::InvalidLength)?
        & !(PAGE_SIZE - 1);
    if new_len_aligned < old_len_aligned {
        return Ok(MremapZeroCopy::Unsupported);
    }
    let old_end = old_addr
        .checked_add(old_len_aligned as u64)
        .ok_or(MmapError::InvalidLength)?;
    let old_start = VirtAddr::new(old_addr);

    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

    let old_vma_ptr = user_as.find_vma(old_start).ok_or(MmapError::NotMapped)?;
    let old_vma = unsafe { &*old_vma_ptr };
    if old_vma.start.as_u64() != old_addr || old_vma.size() != old_len_aligned {
        return Ok(MremapZeroCopy::Unsupported);
    }
    if new_len_aligned > old_len_aligned && old_vma.flags.contains(VmaFlags::DONTEXPAND) {
        return Err(MmapError::PermissionDenied);
    }

    let target_start = if flags & MREMAP_FIXED != 0 {
        VirtAddr::new(new_addr)
    } else {
        user_as
            .find_free_gap(new_len_aligned, None)
            .ok_or(MmapError::OutOfVirtualMemory)?
    };
    let target_end_raw = target_start
        .as_u64()
        .checked_add(new_len_aligned as u64)
        .ok_or(MmapError::OutOfVirtualMemory)?;
    let user_end = crate::memory::core::layout::USER_END.as_u64();
    if target_start.as_u64() < PAGE_SIZE as u64 || target_end_raw > user_end {
        return Err(MmapError::InvalidAddress);
    }
    if flags & MREMAP_FIXED != 0 && target_start.as_u64() < old_end && target_end_raw > old_addr {
        return Err(MmapError::InvalidAddress);
    }
    let target_end = VirtAddr::new(target_end_raw);

    let mut moved_vma = old_vma.clone_metadata();
    moved_vma.start = target_start;
    moved_vma.end = target_end;

    let new_vma = Box::new(moved_vma);
    let new_vma_ptr = Box::into_raw(new_vma);
    if !unsafe { user_as.insert_vma(new_vma_ptr) } {
        let _ = unsafe { Box::from_raw(new_vma_ptr) };
        return Err(MmapError::InvalidAddress);
    }

    let pt_alloc = PtAllocOnly;
    let mut walker = crate::memory::virt::page_table::PageTableWalker::new(user_as.pml4_phys());
    let page_count = old_len_aligned / PAGE_SIZE;
    let mut moved_pages = 0usize;

    for page_idx in 0..page_count {
        let src = VirtAddr::new(old_start.as_u64() + (page_idx * PAGE_SIZE) as u64);
        let dst = VirtAddr::new(target_start.as_u64() + (page_idx * PAGE_SIZE) as u64);
        match walker.move_leaf(src, dst, &pt_alloc) {
            Ok(_) => moved_pages += 1,
            Err(_) => {
                while moved_pages > 0 {
                    moved_pages -= 1;
                    let rollback_src =
                        VirtAddr::new(target_start.as_u64() + (moved_pages * PAGE_SIZE) as u64);
                    let rollback_dst =
                        VirtAddr::new(old_start.as_u64() + (moved_pages * PAGE_SIZE) as u64);
                    let _ = walker.move_leaf(rollback_src, rollback_dst, &pt_alloc);
                }
                free_removed_vma(user_as, target_start);
                return Err(MmapError::AllocFailed);
            }
        }
    }

    free_removed_vma(user_as, old_start);

    let cpu_count = crate::arch::x86_64::acpi::madt::madt_cpu_count();
    let old_end_raw = old_start
        .as_u64()
        .checked_add(old_len_aligned as u64)
        .ok_or(MmapError::OutOfVirtualMemory)?;
    let old_end = VirtAddr::new(old_end_raw);
    unsafe {
        crate::memory::virt::shootdown_sync(
            crate::memory::virt::TlbFlushType::Range {
                start: old_start,
                end: old_end,
            },
            cpu_count,
        );
        crate::memory::virt::shootdown_sync(
            crate::memory::virt::TlbFlushType::Range {
                start: target_start,
                end: target_end,
            },
            cpu_count,
        );
    }

    Ok(MremapZeroCopy::Moved(target_start.as_u64() as usize))
}

const BRK_BASE: u64 = 0x0000_0001_0000_0000; // 4 GiB

fn align_up_page(addr: u64) -> Option<u64> {
    let mask = PAGE_SIZE as u64 - 1;
    addr.checked_add(mask).map(|value| value & !mask)
}

/// Expands or shrinks user heap (per-process via `UserAddressSpace`).
pub fn do_brk(addr: u64) -> Result<u64, MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

    let heap_base = match user_as.heap_start.load(Ordering::Acquire) {
        0 => BRK_BASE,
        base => base,
    };
    let cur = match user_as.heap_end.load(Ordering::Acquire) {
        0 => heap_base,
        current => current,
    };

    if addr == 0 {
        return Ok(cur);
    }

    if addr < heap_base || addr > USER_MMAP_BASE {
        return Err(MmapError::InvalidAddress);
    }

    let new_brk = align_up_page(addr).ok_or(MmapError::InvalidAddress)?;
    if new_brk < heap_base || new_brk > USER_MMAP_BASE {
        return Err(MmapError::InvalidAddress);
    }

    if user_as.heap_start.load(Ordering::Acquire) == 0 {
        user_as.init_heap_bounds(heap_base);
    }

    if new_brk > cur {
        let start = user_as
            .heap_covered_end_from(VirtAddr::new(cur), VirtAddr::new(new_brk))
            .ok_or(MmapError::PermissionDenied)?;
        if start.as_u64() >= new_brk {
            user_as.heap_end.store(new_brk, Ordering::Release);
            return Ok(new_brk);
        }

        let end = VirtAddr::new(new_brk);
        let page_flags =
            PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE;
        let vma_flags = VmaFlags::READ | VmaFlags::WRITE | VmaFlags::ANONYMOUS | VmaFlags::HEAP;

        let vma = Box::new(VmaDescriptor::new(
            start,
            end,
            vma_flags,
            page_flags,
            VmaBacking::Anonymous,
        ));
        let vma_ptr = Box::into_raw(vma);

        let inserted = unsafe { user_as.insert_vma(vma_ptr) };
        if !inserted {
            let _ = unsafe { Box::from_raw(vma_ptr) };
            return Err(MmapError::OutOfVirtualMemory);
        }
    }

    user_as.heap_end.store(new_brk, Ordering::Release);
    Ok(new_brk)
}
