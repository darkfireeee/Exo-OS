//! `munmap` and batched synchronous SMP TLB shootdown + frame reclaim.

use alloc::boxed::Box;
use core::sync::atomic::Ordering;

use crate::memory::core::{VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::UserAddressSpace;
use crate::memory::virt::vma::VmaBacking;

use super::{get_current_user_as, MmapError};

/// Unmaps the virtual region `[addr, addr + align_up(len))` in the current process.
pub fn do_munmap(addr: u64, len: usize) -> Result<(), MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };
    do_munmap_range_in_as(user_as, addr, len)
}

/// Unmaps a full VMA starting at `addr` in an explicit address space.
pub fn do_munmap_in_as(user_as: &UserAddressSpace, addr: u64) -> Result<(), MmapError> {
    if addr < PAGE_SIZE as u64 || !addr.is_multiple_of(PAGE_SIZE as u64) {
        return Err(MmapError::InvalidAddress);
    }

    user_as.stats.munmap_calls.fetch_add(1, Ordering::Relaxed);

    let removed_vma = user_as
        .remove_vma(VirtAddr::new(addr))
        .ok_or(MmapError::NotMapped)?;
    let (vma_start, vma_end, vma_backing) = unsafe {
        let vma = &*removed_vma;
        (vma.start, vma.end, vma.backing)
    };
    let _ = unsafe { Box::from_raw(removed_vma) };
    unmap_and_shootdown_range(user_as, vma_start, vma_end, vma_backing);
    Ok(())
}

/// Unmaps `[addr, addr + align_up(len))` in an explicit address space,
/// splitting partial VMAs and unmapping across multiple VMAs cleanly.
pub fn do_munmap_range_in_as(
    user_as: &UserAddressSpace,
    addr: u64,
    len: usize,
) -> Result<(), MmapError> {
    if addr < PAGE_SIZE as u64 || !addr.is_multiple_of(PAGE_SIZE as u64) {
        return Err(MmapError::InvalidAddress);
    }
    if len == 0 {
        return Err(MmapError::InvalidLength);
    }
    let len_aligned = len
        .checked_add(PAGE_SIZE - 1)
        .ok_or(MmapError::InvalidLength)?
        & !(PAGE_SIZE - 1);
    let end_raw = addr
        .checked_add(len_aligned as u64)
        .ok_or(MmapError::InvalidLength)?;
    if end_raw > crate::memory::core::layout::USER_END.as_u64() {
        return Err(MmapError::InvalidAddress);
    }

    user_as.stats.munmap_calls.fetch_add(1, Ordering::Relaxed);

    let range_end = VirtAddr::new(end_raw);
    let mut cursor = addr;
    while cursor < end_raw {
        match user_as.unmap_vma_range_next(VirtAddr::new(cursor), range_end) {
            Ok(Some((sub_start, sub_end, vma_backing))) => {
                unmap_and_shootdown_range(user_as, sub_start, sub_end, vma_backing);
                cursor = sub_end.as_u64();
            }
            Ok(None) => break,
            Err(_) => return Err(MmapError::AllocFailed),
        }
    }
    Ok(())
}

fn unmap_and_shootdown_range(
    user_as: &UserAddressSpace,
    range_start: VirtAddr,
    range_end: VirtAddr,
    vma_backing: VmaBacking,
) {
    const BATCH: usize = 64; // 64 × 4 KiB = 256 KiB per batch
    let cpu_count = crate::arch::x86_64::acpi::madt::madt_cpu_count();

    let mut cursor = range_start.as_u64();
    while cursor < range_end.as_u64() {
        let mut frames = [crate::memory::core::types::Frame::containing(
            crate::memory::core::types::PhysAddr::new(0),
        ); BATCH];
        let mut pinned = [false; BATCH];
        let mut count = 0usize;

        {
            let _pt_guard = user_as.lock_pt();
            let walker =
                crate::memory::virt::page_table::PageTableWalker::new(user_as.pml4_phys());

            while cursor < range_end.as_u64() && count < BATCH {
                let va = VirtAddr::new(cursor);
                let is_pinned = crate::memory::virt::page_table::PageTableEntry::from_raw(
                    walker.read_pte_raw(va),
                )
                .is_pinned();
                if let Some(f) = unsafe { user_as.unmap_page(va) } {
                    frames[count] = f;
                    pinned[count] = is_pinned;
                    count += 1;
                }
                cursor += PAGE_SIZE as u64;
            }

            if count == 0 {
                continue;
            }

            let batch_start = VirtAddr::new(
                range_start
                    .as_u64()
                    .max(cursor - (count as u64 * PAGE_SIZE as u64)),
            );
            let batch_end = VirtAddr::new(cursor);
            #[cfg(target_arch = "x86_64")]
            crate::arch::x86_64::exceptions::page_fault::sync_kpti_user_fault_mapping(
                batch_start.as_u64(),
                user_as.pml4_phys().as_u64(),
            );
            unsafe {
                crate::memory::virt::shootdown_sync(
                    crate::memory::virt::TlbFlushType::Range {
                        start: batch_start,
                        end: batch_end,
                    },
                    cpu_count,
                );
            }
        }

        if vma_backing == VmaBacking::Anonymous {
            for i in 0..count {
                if pinned[i] {
                    continue;
                }
                let remaining = crate::memory::cow::tracker::COW_TRACKER.dec(frames[i]);
                if remaining == 0 || remaining == u32::MAX {
                    let _ = crate::memory::physical::allocator::buddy::free_page(frames[i]);
                }
            }
        }
    }
}
