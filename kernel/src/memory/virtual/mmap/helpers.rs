//! Shared types, constants, and internal helpers for `memory::virt::mmap`.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::memory::core::{PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::UserAddressSpace;
use crate::memory::virt::vma::VmaFlags;

/// Possible errors for mmap/munmap/mprotect/brk operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmapError {
    InvalidLength,
    InvalidAddress,
    OutOfVirtualMemory,
    NoAddressSpace,
    AllocFailed,
    NotMapped,
    PermissionDenied,
    Exists,
}

impl MmapError {
    pub fn to_kernel_errno(self) -> i64 {
        match self {
            MmapError::InvalidLength => -22,      // EINVAL
            MmapError::InvalidAddress => -22,     // EINVAL
            MmapError::OutOfVirtualMemory => -12, // ENOMEM
            MmapError::NoAddressSpace => -12,     // ENOMEM
            MmapError::AllocFailed => -12,        // ENOMEM
            MmapError::NotMapped => -14,          // EFAULT
            MmapError::PermissionDenied => -13,   // EACCES
            MmapError::Exists => -17,             // EEXIST
        }
    }
}

/// Internal result of zero-copy mremap path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MremapZeroCopy {
    Moved(usize),
    Unsupported,
}

pub type CurrentAsGetterFn = fn() -> *mut UserAddressSpace;

static CURRENT_AS_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

pub fn register_current_as_getter(f: CurrentAsGetterFn) {
    CURRENT_AS_FN.store(f as *mut (), Ordering::Release);
}

pub(super) fn get_current_user_as() -> Option<*mut UserAddressSpace> {
    let ptr = CURRENT_AS_FN.load(Ordering::Acquire);
    if !ptr.is_null() {
        // SAFETY: Registered via register_current_as_getter with expected signature.
        let f: CurrentAsGetterFn = unsafe { core::mem::transmute(ptr) };
        let as_ptr = f();
        if !as_ptr.is_null() {
            return Some(as_ptr);
        }
    }
    let tcb_raw = unsafe { crate::arch::x86_64::smp::percpu::read_current_tcb() };
    if tcb_raw == 0 {
        return None;
    }
    let tcb = unsafe { &*(tcb_raw as *const crate::scheduler::core::task::ThreadControlBlock) };
    let pid = crate::process::core::pid::Pid(tcb.pid.0);
    let pcb = crate::process::core::registry::PROCESS_REGISTRY.find_by_pid(pid)?;
    let as_ptr = pcb.address_space_ptr() as *mut UserAddressSpace;
    if as_ptr.is_null() {
        None
    } else {
        Some(as_ptr)
    }
}

pub(super) const PROT_READ: u32 = 1;
pub(super) const PROT_WRITE: u32 = 2;
pub(super) const PROT_EXEC: u32 = 4;
const KNOWN_PROT: u32 = PROT_READ | PROT_WRITE | PROT_EXEC;

pub(super) const MAP_SHARED: u32 = 0x01;
#[allow(dead_code)]
pub(super) const MAP_PRIVATE: u32 = 0x02;
pub(super) const MAP_FIXED: u32 = 0x10;
pub(super) const MAP_ANON: u32 = 0x20;
pub(super) const MAP_FIXED_NOREPLACE: u32 = 0x10_0000;

pub(super) struct PtAllocOnly;
impl crate::memory::virt::page_table::FrameAllocatorForWalk for PtAllocOnly {
    fn alloc_frame(
        &self,
        flags: crate::memory::AllocFlags,
    ) -> Result<crate::memory::Frame, crate::memory::AllocError> {
        crate::memory::physical::allocator::buddy::alloc_pages(0, flags)
    }
    fn free_frame(&self, f: crate::memory::Frame) {
        let _ = crate::memory::physical::allocator::buddy::free_pages(f, 0);
    }
}

pub(super) fn prot_to_page_flags(prot: u32) -> PageFlags {
    if prot & (PROT_READ | PROT_WRITE | PROT_EXEC) == 0 {
        return PageFlags::PROT_NONE | PageFlags::USER | PageFlags::NO_EXECUTE;
    }
    let mut f = PageFlags::PRESENT | PageFlags::USER;
    if prot & PROT_WRITE != 0 {
        f |= PageFlags::WRITABLE;
    }
    if prot & PROT_EXEC == 0 {
        f |= PageFlags::NO_EXECUTE;
    }
    f
}

pub(super) fn prot_to_vma_flags(prot: u32, map_flags: u32) -> VmaFlags {
    let mut f = VmaFlags::NONE;
    if prot & PROT_READ != 0 {
        f |= VmaFlags::READ;
    }
    if prot & PROT_WRITE != 0 {
        f |= VmaFlags::WRITE;
    }
    if prot & PROT_EXEC != 0 {
        f |= VmaFlags::EXEC;
    }
    if map_flags & MAP_SHARED != 0 {
        f |= VmaFlags::SHARED;
    }
    if map_flags & (MAP_FIXED | MAP_FIXED_NOREPLACE) != 0 {
        f |= VmaFlags::FIXED;
    }
    if map_flags & MAP_ANON != 0 {
        f |= VmaFlags::ANONYMOUS;
    }
    f
}

#[inline]
pub(super) fn validate_prot(prot: u32) -> Result<(), MmapError> {
    if prot & !KNOWN_PROT != 0 {
        return Err(MmapError::InvalidAddress);
    }
    if prot & PROT_WRITE != 0 && prot & PROT_EXEC != 0 {
        return Err(MmapError::PermissionDenied);
    }
    Ok(())
}

pub(super) fn record_exec_mapping_denied(reason_code: u32, addr_or_oid: u64) {
    let pid = crate::syscall::fast_path::syscall_current_pid();
    crate::security::exoledger::exo_ledger_append(
        crate::security::exoledger::ActionTag::AccessDenied {
            oid: addr_or_oid,
            rights: PROT_EXEC,
        },
    );
    crate::security::shield_feed::push_event(
        pid,
        crate::security::shield_feed::event_type::MEMORY,
        crate::security::shield_feed::severity::HIGH,
        reason_code,
        addr_or_oid,
        PROT_EXEC as u64,
    );
}

pub(super) fn free_removed_vma(user_as: &UserAddressSpace, start: VirtAddr) {
    if let Some(vma_ptr) = user_as.remove_vma(start) {
        let _ = unsafe { Box::from_raw(vma_ptr) };
    }
}

pub(super) fn rollback_shm_into_process(
    user_as: &UserAddressSpace,
    virt_base: VirtAddr,
    n_pages: usize,
) {
    for i in 0..n_pages {
        let v = VirtAddr::new(virt_base.as_u64().saturating_add((i * PAGE_SIZE) as u64));
        unsafe {
            user_as.unmap_page(v);
        }
    }
    free_removed_vma(user_as, virt_base);
}

pub(super) fn rollback_anon_pages(
    user_as: &UserAddressSpace,
    virt_base: VirtAddr,
    n_pages: usize,
) {
    for i in 0..n_pages {
        let v = VirtAddr::new(virt_base.as_u64().saturating_add((i * PAGE_SIZE) as u64));
        if let Some(frame) = unsafe { user_as.unmap_page(v) } {
            let _ = crate::memory::physical::allocator::buddy::free_pages(frame, 0);
        }
    }
    free_removed_vma(user_as, virt_base);
}
