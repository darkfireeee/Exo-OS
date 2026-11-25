//! `mprotect` permission transitions, W^X/S2 enforcement, and CoW preservation.

use crate::memory::core::{PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::UserAddressSpace;
use crate::memory::virt::vma::VmaFlags;

use super::{
    get_current_user_as, prot_to_page_flags, record_exec_mapping_denied, validate_prot, MmapError,
    PtAllocOnly, PROT_EXEC, PROT_READ, PROT_WRITE,
};

/// Modifies permissions of an existing memory region.
pub fn do_mprotect(addr: u64, len: usize, prot: u32) -> Result<(), MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };
    do_mprotect_in_as_inner(user_as, addr, len, prot, false)
}

/// Internal helper invoked exclusively by `fs_mmap_file` after file-backed
/// signature / trust verification succeeds (S2). Marks the VMA `EXEC_VERIFIED`.
pub fn do_mprotect_verified_file_exec(addr: u64, len: usize, prot: u32) -> Result<(), MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };
    do_mprotect_in_as_inner(user_as, addr, len, prot, true)
}

/// Computes the leaf PTE flags for `mprotect`, preserving CoW tracking across `PROT_NONE`.
pub(super) fn resolve_mprotect_pte_flags(
    old_entry: crate::memory::virt::page_table::PageTableEntry,
    eff_page_flags: PageFlags,
    is_prot_none: bool,
    is_shared: bool,
    is_tracked_cow: bool,
) -> PageFlags {
    let is_shared_cow = !is_shared && (old_entry.is_cow() || is_tracked_cow);
    if is_prot_none && (old_entry.is_cow() || is_shared_cow) {
        eff_page_flags.set(PageFlags::COW)
    } else if is_shared_cow && eff_page_flags.contains(PageFlags::WRITABLE) {
        eff_page_flags
            .clear(PageFlags::WRITABLE)
            .set(PageFlags::COW)
    } else {
        eff_page_flags
    }
}

/// Modifies permissions of a region in an explicit address space.
pub fn do_mprotect_in_as(
    user_as: &UserAddressSpace,
    addr: u64,
    len: usize,
    prot: u32,
) -> Result<(), MmapError> {
    do_mprotect_in_as_inner(user_as, addr, len, prot, false)
}

fn do_mprotect_in_as_inner(
    user_as: &UserAddressSpace,
    addr: u64,
    len: usize,
    prot: u32,
    verified_file_exec: bool,
) -> Result<(), MmapError> {
    if addr < PAGE_SIZE as u64 || !addr.is_multiple_of(PAGE_SIZE as u64) {
        return Err(MmapError::InvalidAddress);
    }
    if len == 0 {
        return Err(MmapError::InvalidLength);
    }
    validate_prot(prot)?;

    let len_aligned = len
        .checked_add(PAGE_SIZE - 1)
        .ok_or(MmapError::InvalidLength)?
        & !(PAGE_SIZE - 1);
    let range_start = VirtAddr::new(addr);
    let range_end_raw = addr
        .checked_add(len_aligned as u64)
        .ok_or(MmapError::InvalidLength)?;
    if range_end_raw > crate::memory::core::layout::USER_END.as_u64() {
        return Err(MmapError::InvalidAddress);
    }
    let range_end = VirtAddr::new(range_end_raw);

    if !user_as.verify_vma_range_covered(range_start, range_end) {
        return Err(MmapError::NotMapped);
    }

    // S2: Reject unsigned `RW -> RX` transitions on unverified / anonymous VMAs.
    if (prot & PROT_EXEC) != 0
        && !verified_file_exec
        && !user_as.verify_vma_range_exec_verified(range_start, range_end)
    {
        record_exec_mapping_denied(2, addr);
        return Err(MmapError::PermissionDenied);
    }

    let base_page_flags = prot_to_page_flags(prot);
    let mut new_perm = VmaFlags::NONE;
    if prot & PROT_READ != 0 {
        new_perm |= VmaFlags::READ;
    }
    if prot & PROT_WRITE != 0 {
        new_perm |= VmaFlags::WRITE;
    }
    if prot & PROT_EXEC != 0 {
        new_perm |= VmaFlags::EXEC;
        if verified_file_exec {
            new_perm |= VmaFlags::EXEC_VERIFIED;
        }
    }

    let is_prot_none = prot & (PROT_READ | PROT_WRITE | PROT_EXEC) == 0;
    let cpu_count = crate::arch::x86_64::acpi::madt::madt_cpu_count();
    let mut cursor = addr;
    while cursor < range_end_raw {
        let (sub_start, sub_end, eff_page_flags, is_shared) = match user_as
            .protect_vma_range_next(VirtAddr::new(cursor), range_end, base_page_flags, new_perm)
        {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(true) => return Err(MmapError::AllocFailed),
            Err(false) => return Err(MmapError::NotMapped),
        };

        {
            let pt_alloc = PtAllocOnly;
            let _pt_guard = user_as.lock_pt();
            let mut walker =
                crate::memory::virt::page_table::PageTableWalker::new(user_as.pml4_phys());
            let mut pt_cursor = sub_start.as_u64();
            while pt_cursor < sub_end.as_u64() {
                let va = VirtAddr::new(pt_cursor);
                let old_raw = walker.read_pte_raw(va);
                let old_entry =
                    crate::memory::virt::page_table::PageTableEntry::from_raw(old_raw);
                if old_entry.is_present() || old_entry.is_prot_none() {
                    let is_tracked_cow = old_entry.frame_or_prot_none().is_some_and(|f| {
                        crate::memory::cow::tracker::COW_TRACKER
                            .tracked_ref_count(f)
                            .is_some_and(|rc| rc > 1)
                    });
                    let pte_flags = resolve_mprotect_pte_flags(
                        old_entry,
                        eff_page_flags,
                        is_prot_none,
                        is_shared,
                        is_tracked_cow,
                    );
                    let _ = walker.remap_flags(va, pte_flags);
                } else if is_prot_none {
                    let _ = walker.set_guard_pte(va, &pt_alloc);
                } else if crate::memory::integrity::guard_pages::is_guard_pte(old_raw) {
                    walker.clear_guard_pte(va);
                }
                pt_cursor += PAGE_SIZE as u64;
            }

            #[cfg(target_arch = "x86_64")]
            crate::arch::x86_64::exceptions::page_fault::sync_kpti_user_fault_mapping(
                sub_start.as_u64(),
                user_as.pml4_phys().as_u64(),
            );
            unsafe {
                crate::memory::virt::shootdown_sync(
                    crate::memory::virt::TlbFlushType::Range {
                        start: sub_start,
                        end: sub_end,
                    },
                    cpu_count,
                );
            }
        }

        cursor = sub_end.as_u64();
    }

    Ok(())
}
