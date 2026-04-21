// kernel/src/memory/virtual/address_space/fork_impl.rs
//
// Implémentation de AddressSpaceCloner pour l'espace utilisateur.
// CORRECTION P0-01 : débloque fork() en clonant l'espace d'adressage en CoW.
//
// Cette implémentation :
//   1. Alloue un nouveau PML4 pour le processus fils
//   2. Copie les entrées kernel (PML4[256:512]) depuis le parent
//   3. Copie les pages userspace et les marque CoW
//   4. Fournit les primitives de libération pour les chemins d'erreur

use crate::process::lifecycle::fork::{
    AddressSpaceCloner, ClonedAddressSpace, AddrSpaceCloneError,
};
use crate::memory::cow::tracker::COW_TRACKER;
use crate::memory::virt::address_space::tlb::{shootdown_sync, TlbFlushType};
use crate::memory::virt::page_table::x86_64::{
    phys_to_table_mut, phys_to_table_ref, PageTableEntry,
};
use crate::memory::virt::page_table::FrameAllocatorForWalk;
use crate::memory::virt::UserAddressSpace;
use crate::memory::core::{PhysAddr, Frame};
use crate::memory::AllocFlags;
use crate::memory::physical::allocator::buddy;

pub struct KernelAddressSpaceCloner;

struct ForkWalkAllocator;

unsafe impl Send for KernelAddressSpaceCloner {}
unsafe impl Sync for KernelAddressSpaceCloner {}

impl FrameAllocatorForWalk for ForkWalkAllocator {
    fn alloc_frame(&self, flags: AllocFlags) -> Result<Frame, crate::memory::AllocError> {
        buddy::alloc_pages(0, flags)
    }

    fn free_frame(&self, frame: Frame) {
        let _ = buddy::free_pages(frame, 0);
    }
}

impl AddressSpaceCloner for KernelAddressSpaceCloner {
    fn clone_cow(
        &self,
        src_cr3:       u64,
        src_space_ptr: usize,
    ) -> Result<ClonedAddressSpace, AddrSpaceCloneError> {
        if src_cr3 == 0 {
            return Err(AddrSpaceCloneError::InvalidSource);
        }
        let child_pml4_frame = buddy::alloc_pages(0, crate::memory::AllocFlags::ZEROED)
            .map_err(|_| AddrSpaceCloneError::OutOfMemory)?;
        let child_cr3 = child_pml4_frame.start_address().as_u64();
        let child_pml4_phys = PhysAddr::new(child_cr3);

        unsafe {
            let src_pml4 = phys_to_table_ref(PhysAddr::new(src_cr3));
            let dst_pml4 = phys_to_table_mut(child_pml4_phys);
            for i in 256..512 {
                dst_pml4[i] = src_pml4[i];
            }
            if clone_userspace_tables(PhysAddr::new(src_cr3), child_pml4_phys).is_err() {
                free_userspace_tables(child_pml4_phys);
                return Err(AddrSpaceCloneError::OutOfMemory);
            }
        }

        let mut child_as = Box::new(UserAddressSpace::new(child_pml4_phys, 0));
        if src_space_ptr != 0 {
            let parent_as = unsafe { &*(src_space_ptr as *const UserAddressSpace) };
            child_as.heap_end.store(
                parent_as.heap_end.load(core::sync::atomic::Ordering::Acquire),
                core::sync::atomic::Ordering::Release,
            );
        }

        Ok(ClonedAddressSpace {
            cr3:             child_cr3,
            addr_space_ptr:  Box::into_raw(child_as) as usize,
        })
    }

    fn flush_tlb_after_fork(&self, _parent_cr3: u64) {
        unsafe {
            shootdown_sync(
                TlbFlushType::All,
                crate::arch::x86_64::smp::init::smp_cpu_count(),
            );
        }
    }

    fn free_addr_space(&self, addr_space_ptr: usize) {
        if addr_space_ptr == 0 {
            return;
        }
        let addr_space = unsafe { Box::from_raw(addr_space_ptr as *mut UserAddressSpace) };
        unsafe { free_userspace_tables(addr_space.pml4_phys()); }
    }
}

/// Instance statique du clonage d'espace d'adressage.
pub static KERNEL_AS_CLONER: KernelAddressSpaceCloner = KernelAddressSpaceCloner;

unsafe fn clone_userspace_tables(
    src_pml4_phys: PhysAddr,
    dst_pml4_phys: PhysAddr,
) -> Result<(), AddrSpaceCloneError> {
    let src_pml4 = phys_to_table_ref(src_pml4_phys);
    let dst_pml4 = phys_to_table_mut(dst_pml4_phys);

    for l4_idx in 0..256 {
        let src_entry = src_pml4[l4_idx];
        if !src_entry.is_present() {
            continue;
        }

        let dst_pdpt_phys = alloc_zeroed_table()?;
        clone_pdpt(src_entry.phys_addr(), dst_pdpt_phys)?;
        dst_pml4[l4_idx] = repoint_table_entry(src_entry, dst_pdpt_phys);
    }

    Ok(())
}

unsafe fn clone_pdpt(
    src_pdpt_phys: PhysAddr,
    dst_pdpt_phys: PhysAddr,
) -> Result<(), AddrSpaceCloneError> {
    let src_pdpt = phys_to_table_mut(src_pdpt_phys);
    let dst_pdpt = phys_to_table_mut(dst_pdpt_phys);

    for l3_idx in 0..512 {
        let src_entry = src_pdpt[l3_idx];
        if !src_entry.is_present() {
            continue;
        }
        if src_entry.is_huge() {
            if let Some(frame) = src_entry.frame() {
                let _ = COW_TRACKER.inc(frame);
            }
            let shared = shared_entry(src_entry);
            src_pdpt[l3_idx] = shared;
            dst_pdpt[l3_idx] = shared;
            continue;
        }

        let dst_pd_phys = alloc_zeroed_table()?;
        clone_pd(src_entry.phys_addr(), dst_pd_phys)?;
        dst_pdpt[l3_idx] = repoint_table_entry(src_entry, dst_pd_phys);
    }

    Ok(())
}

unsafe fn clone_pd(
    src_pd_phys: PhysAddr,
    dst_pd_phys: PhysAddr,
) -> Result<(), AddrSpaceCloneError> {
    let src_pd = phys_to_table_mut(src_pd_phys);
    let dst_pd = phys_to_table_mut(dst_pd_phys);

    for l2_idx in 0..512 {
        let src_entry = src_pd[l2_idx];
        if !src_entry.is_present() {
            continue;
        }
        if src_entry.is_huge() {
            if let Some(frame) = src_entry.frame() {
                let _ = COW_TRACKER.inc(frame);
            }
            let shared = shared_entry(src_entry);
            src_pd[l2_idx] = shared;
            dst_pd[l2_idx] = shared;
            continue;
        }

        let dst_pt_phys = alloc_zeroed_table()?;
        clone_pt(src_entry.phys_addr(), dst_pt_phys);
        dst_pd[l2_idx] = repoint_table_entry(src_entry, dst_pt_phys);
    }

    Ok(())
}

unsafe fn clone_pt(src_pt_phys: PhysAddr, dst_pt_phys: PhysAddr) {
    let src_pt = phys_to_table_mut(src_pt_phys);
    let dst_pt = phys_to_table_mut(dst_pt_phys);

    for l1_idx in 0..512 {
        let src_entry = src_pt[l1_idx];
        if src_entry.is_present() {
            if let Some(frame) = src_entry.frame() {
                let _ = COW_TRACKER.inc(frame);
            }
            let shared = shared_entry(src_entry);
            src_pt[l1_idx] = shared;
            dst_pt[l1_idx] = shared;
        }
    }
}

#[inline]
fn shared_entry(src_entry: PageTableEntry) -> PageTableEntry {
    if src_entry.is_writable() {
        PageTableEntry::from_raw(
            (src_entry.raw() & !PageTableEntry::FLAG_WRITABLE) | PageTableEntry::FLAG_COW,
        )
    } else {
        src_entry
    }
}

fn alloc_zeroed_table() -> Result<PhysAddr, AddrSpaceCloneError> {
    buddy::alloc_pages(0, crate::memory::AllocFlags::ZEROED)
        .map(|frame| frame.start_address())
        .map_err(|_| AddrSpaceCloneError::OutOfMemory)
}

fn repoint_table_entry(src_entry: PageTableEntry, new_phys: PhysAddr) -> PageTableEntry {
    const ENTRY_FLAG_MASK: u64 = !0x000F_FFFF_FFFF_F000u64;
    PageTableEntry::from_raw(new_phys.as_u64() | (src_entry.raw() & ENTRY_FLAG_MASK))
}

unsafe fn free_userspace_tables(root_pml4_phys: PhysAddr) {
    let pml4 = phys_to_table_ref(root_pml4_phys);
    for l4_idx in 0..256 {
        let entry = pml4[l4_idx];
        if entry.is_present() && !entry.is_huge() {
            free_pdpt_tree(entry.phys_addr());
        }
    }
    let _ = buddy::free_pages(Frame::containing(root_pml4_phys), 0);
}

unsafe fn free_pdpt_tree(pdpt_phys: PhysAddr) {
    let pdpt = phys_to_table_ref(pdpt_phys);
    for l3_idx in 0..512 {
        let entry = pdpt[l3_idx];
        if !entry.is_present() {
            continue;
        }
        if entry.is_huge() {
            release_huge_frame(entry, 18);
        } else {
            free_pd_tree(entry.phys_addr());
        }
    }
    let _ = buddy::free_pages(Frame::containing(pdpt_phys), 0);
}

unsafe fn free_pd_tree(pd_phys: PhysAddr) {
    let pd = phys_to_table_ref(pd_phys);
    for l2_idx in 0..512 {
        let entry = pd[l2_idx];
        if !entry.is_present() {
            continue;
        }
        if entry.is_huge() {
            release_huge_frame(entry, 9);
        } else {
            free_pt_tree(entry.phys_addr());
        }
    }
    let _ = buddy::free_pages(Frame::containing(pd_phys), 0);
}

unsafe fn free_pt_tree(pt_phys: PhysAddr) {
    let pt = phys_to_table_ref(pt_phys);
    for l1_idx in 0..512 {
        let entry = pt[l1_idx];
        if !entry.is_present() {
            continue;
        }
        release_leaf_frame(entry);
    }
    let _ = buddy::free_pages(Frame::containing(pt_phys), 0);
}

fn release_leaf_frame(entry: PageTableEntry) {
    let Some(frame) = entry.frame() else {
        return;
    };
    let remaining = COW_TRACKER.dec(frame);
    if remaining == 0 {
        let _ = buddy::free_pages(frame, 0);
    }
}

fn release_huge_frame(entry: PageTableEntry, order: usize) {
    let Some(frame) = entry.frame() else {
        return;
    };
    let remaining = COW_TRACKER.dec(frame);
    if remaining == 0 {
        let _ = buddy::free_pages(frame, order);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_entry_turns_writable_page_into_cow() {
        let frame = Frame::containing(PhysAddr::new(0x20_000));
        let entry = PageTableEntry::new(
            frame,
            PageTableEntry::FLAG_PRESENT | PageTableEntry::FLAG_WRITABLE | PageTableEntry::FLAG_USER,
        );

        let shared = shared_entry(entry);

        assert!(shared.is_present());
        assert!(shared.is_user());
        assert!(shared.is_cow());
        assert!(!shared.is_writable());
        assert_eq!(shared.phys_addr().as_u64(), entry.phys_addr().as_u64());
    }

    #[test]
    fn shared_entry_keeps_read_only_mapping_unchanged() {
        let frame = Frame::containing(PhysAddr::new(0x24_000));
        let entry = PageTableEntry::new(
            frame,
            PageTableEntry::FLAG_PRESENT | PageTableEntry::FLAG_USER,
        );

        let shared = shared_entry(entry);

        assert_eq!(shared.raw(), entry.raw());
    }
}
