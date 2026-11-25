//! Unit tests for `memory::virt::mmap`.

use alloc::boxed::Box;

use crate::memory::core::{Frame, PageFlags, PhysAddr, VirtAddr, PAGE_SIZE};
use crate::memory::integrity::guard_pages::{is_guard_pte, GUARD_PTE_VALUE};
use crate::memory::virt::address_space::UserAddressSpace;
use crate::memory::virt::page_table::PageTableEntry;
use crate::memory::virt::vma::{VmaBacking, VmaDescriptor, VmaFlags};

use super::anon_shared::{insert_vma_replacing_fixed, resolve_mmap_range};
use super::mprotect::resolve_mprotect_pte_flags;
use super::{
    prot_to_page_flags, MmapError, MAP_FIXED, MAP_FIXED_NOREPLACE, PROT_READ, PROT_WRITE,
};

#[test]
fn prot_none_flags_omit_present_and_preserve_frame_and_cow_roundtrip() {
    let none_flags = prot_to_page_flags(0);
    assert!(none_flags.contains(PageFlags::PROT_NONE));
    assert!(none_flags.contains(PageFlags::USER));
    assert!(none_flags.contains(PageFlags::NO_EXECUTE));
    assert!(!none_flags.contains(PageFlags::PRESENT));
    assert!(!none_flags.contains(PageFlags::WRITABLE));

    let rw_flags = prot_to_page_flags(PROT_READ | PROT_WRITE);
    assert!(rw_flags.contains(PageFlags::PRESENT));
    assert!(rw_flags.contains(PageFlags::USER));
    assert!(rw_flags.contains(PageFlags::WRITABLE));
    assert!(rw_flags.contains(PageFlags::NO_EXECUTE));
    assert!(!rw_flags.contains(PageFlags::PROT_NONE));

    let frame = Frame::containing(PhysAddr::new(0x40_000));
    let rw_cow_entry = PageTableEntry::from_page_flags(
        frame,
        PageFlags::PRESENT | PageFlags::USER | PageFlags::COW | PageFlags::NO_EXECUTE,
    );
    let masked_flags = resolve_mprotect_pte_flags(rw_cow_entry, none_flags, true, false, true);
    assert!(masked_flags.contains(PageFlags::PROT_NONE));
    assert!(masked_flags.contains(PageFlags::COW));
    assert!(!masked_flags.contains(PageFlags::PRESENT));

    let masked_entry = PageTableEntry::from_page_flags(frame, masked_flags);
    assert!(!masked_entry.is_present());
    assert!(masked_entry.is_prot_none());
    assert!(masked_entry.is_cow());
    assert_eq!(masked_entry.frame_or_prot_none(), Some(frame));

    let restored_flags = resolve_mprotect_pte_flags(masked_entry, rw_flags, false, false, true);
    assert!(restored_flags.contains(PageFlags::PRESENT));
    assert!(restored_flags.contains(PageFlags::COW));
    assert!(!restored_flags.contains(PageFlags::WRITABLE));
    assert!(!restored_flags.contains(PageFlags::PROT_NONE));

    let restored_entry = PageTableEntry::from_page_flags(frame, restored_flags);
    assert!(restored_entry.is_present());
    assert!(restored_entry.is_cow());
    assert!(!restored_entry.is_writable());
    assert_eq!(restored_entry.frame_or_prot_none(), Some(frame));
}

#[test]
fn guard_pte_value_distinct_from_prot_none_entry() {
    let frame = Frame::containing(PhysAddr::new(0x50_000));
    let prot_none_entry = PageTableEntry::from_page_flags(frame, prot_to_page_flags(0));
    assert!(prot_none_entry.is_prot_none());
    assert!(!is_guard_pte(prot_none_entry.raw()));
    assert!(is_guard_pte(GUARD_PTE_VALUE));
    assert!(!PageTableEntry::from_raw(GUARD_PTE_VALUE).is_prot_none());
    assert_eq!(prot_none_entry.frame_or_prot_none(), Some(frame));
}

#[test]
fn resolve_mmap_range_and_fixed_noreplace_respect_bounds() {
    let user_as = UserAddressSpace::new(PhysAddr::new(0x1000), 0);
    assert_eq!(
        resolve_mmap_range(&user_as, 0, PAGE_SIZE, MAP_FIXED),
        Err(MmapError::InvalidAddress)
    );
    assert_eq!(
        resolve_mmap_range(&user_as, 0x10001, PAGE_SIZE, MAP_FIXED),
        Err(MmapError::InvalidAddress)
    );
    let (s, e) = resolve_mmap_range(&user_as, 0x2000_0000, PAGE_SIZE * 4, MAP_FIXED).unwrap();
    assert_eq!(s.as_u64(), 0x2000_0000);
    assert_eq!(e.as_u64(), 0x2000_4000);

    let vma = Box::new(VmaDescriptor::new(
        s,
        e,
        VmaFlags::READ | VmaFlags::WRITE | VmaFlags::ANONYMOUS,
        PageFlags::USER_DATA,
        VmaBacking::Anonymous,
    ));
    assert!(unsafe { user_as.insert_vma(Box::into_raw(vma)) });

    let overlap_vma = Box::into_raw(Box::new(VmaDescriptor::new(
        VirtAddr::new(0x2000_1000),
        VirtAddr::new(0x2000_2000),
        VmaFlags::READ | VmaFlags::FIXED | VmaFlags::ANONYMOUS,
        PageFlags::USER | PageFlags::PRESENT | PageFlags::NO_EXECUTE,
        VmaBacking::Anonymous,
    )));
    assert_eq!(
        insert_vma_replacing_fixed(
            &user_as,
            VirtAddr::new(0x2000_1000),
            PAGE_SIZE,
            MAP_FIXED | MAP_FIXED_NOREPLACE,
            overlap_vma,
        ),
        Err(MmapError::Exists)
    );
    assert_eq!(MmapError::Exists.to_kernel_errno(), -17);

    let unmapped = user_as
        .unmap_vma_range_next(VirtAddr::new(0x2000_1000), VirtAddr::new(0x2000_3000))
        .unwrap();
    assert_eq!(
        unmapped,
        Some((
            VirtAddr::new(0x2000_1000),
            VirtAddr::new(0x2000_3000),
            VmaBacking::Anonymous
        ))
    );
    assert!(user_as.verify_vma_range_covered(VirtAddr::new(0x2000_0000), VirtAddr::new(0x2000_1000)));
    assert!(!user_as.verify_vma_range_covered(VirtAddr::new(0x2000_1000), VirtAddr::new(0x2000_3000)));
    assert!(user_as.verify_vma_range_covered(VirtAddr::new(0x2000_3000), VirtAddr::new(0x2000_4000)));
}
