// kernel/src/memory/virtual/mmap.rs
//
// ──────────────────────────────────────────────────────────────────────────────
// IMPLEMENTATION OF MMAP / MUNMAP / MPROTECT / BRK
// ──────────────────────────────────────────────────────────────────────────────
//
// Implements user virtual memory management system calls.
// The syscall layer injects a function pointer to the current
// address space via `register_current_as_getter()`.
//
// LAYER 0 — no scheduler/process/ipc/fs dependencies.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::memory::core::{PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::virt::address_space::{UserAddressSpace, USER_MMAP_BASE};
use crate::memory::virt::vma::{VmaBacking, VmaDescriptor, VmaFlags};

// ─────────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────────

/// Possible errors for mmap/munmap/mprotect/brk operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmapError {
    /// Invalid length (0 or overflow).
    InvalidLength,
    /// Invalid or unaligned address.
    InvalidAddress,
    /// Out of virtual memory space.
    OutOfVirtualMemory,
    /// No current address space (getter not registered or null).
    NoAddressSpace,
    /// Memory allocation failure.
    AllocFailed,
    /// Region not mapped.
    NotMapped,
    /// Permission denied.
    PermissionDenied,
}

impl MmapError {
    /// Translates the error into a negative POSIX errno.
    pub fn to_kernel_errno(self) -> i64 {
        match self {
            MmapError::InvalidLength => -22,      // EINVAL
            MmapError::InvalidAddress => -22,     // EINVAL
            MmapError::OutOfVirtualMemory => -12, // ENOMEM
            MmapError::NoAddressSpace => -12,     // ENOMEM
            MmapError::AllocFailed => -12,        // ENOMEM
            MmapError::NotMapped => -14,          // EFAULT
            MmapError::PermissionDenied => -13,   // EACCES
        }
    }
}

/// Internal result of zero-copy mremap path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MremapZeroCopy {
    /// The VMA was moved by moving PTEs.
    Moved(usize),
    /// The requested case requires the copy compatibility fallback.
    Unsupported,
}

// ─────────────────────────────────────────────────────────────────────────────
// Injection of current address space getter (LAYER 0 pattern)
// ─────────────────────────────────────────────────────────────────────────────

/// Signature of function returning the current thread's UserAddressSpace.
pub type CurrentAsGetterFn = fn() -> *mut UserAddressSpace;

/// Function pointer registered by the upper layer (scheduler/process).
static CURRENT_AS_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Registers the function returning the current thread's address space.
/// Must be called by the scheduler during its initialization.
pub fn register_current_as_getter(f: CurrentAsGetterFn) {
    CURRENT_AS_FN.store(f as *mut (), Ordering::Release);
}

/// Gets a pointer to the current user address space.
fn get_current_user_as() -> Option<*mut UserAddressSpace> {
    let ptr = CURRENT_AS_FN.load(Ordering::Acquire);
    if !ptr.is_null() {
        // SAFETY: Registered via register_current_as_getter with expected signature.
        let f: CurrentAsGetterFn = unsafe { core::mem::transmute(ptr) };
        let as_ptr = f();
        if !as_ptr.is_null() {
            return Some(as_ptr);
        }
    }
    // Direct fallback via current thread's PCB
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

// ─────────────────────────────────────────────────────────────────────────────
// POSIX constants (prot / map flags)
// ─────────────────────────────────────────────────────────────────────────────

/// Read protection (PROT_READ).
const PROT_READ: u32 = 1;
/// Write protection (PROT_WRITE).
const PROT_WRITE: u32 = 2;
/// Execute protection (PROT_EXEC).
const PROT_EXEC: u32 = 4;
const KNOWN_PROT: u32 = PROT_READ | PROT_WRITE | PROT_EXEC;

/// Shared mapping (MAP_SHARED).
const MAP_SHARED: u32 = 0x01;
/// Private mapping (MAP_PRIVATE).
#[allow(dead_code)]
const MAP_PRIVATE: u32 = 0x02;
/// Fixed address (MAP_FIXED).
const MAP_FIXED: u32 = 0x10;
/// Anonymous mapping (MAP_ANONYMOUS / MAP_ANON).
const MAP_ANON: u32 = 0x20;

// ─────────────────────────────────────────────────────────────────────────────
// Flag conversion helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Converts POSIX protections to x86 page flags.
fn prot_to_page_flags(prot: u32) -> PageFlags {
    let mut f = PageFlags::PRESENT | PageFlags::USER;
    if prot & PROT_WRITE != 0 {
        f |= PageFlags::WRITABLE;
    }
    // NX by default unless explicit PROT_EXEC
    if prot & PROT_EXEC == 0 {
        f |= PageFlags::NO_EXECUTE;
    }
    f
}

/// Converts POSIX flags (prot + map_flags) to VmaFlags.
fn prot_to_vma_flags(prot: u32, map_flags: u32) -> VmaFlags {
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
    if map_flags & MAP_FIXED != 0 {
        f |= VmaFlags::FIXED;
    }
    if map_flags & MAP_ANON != 0 {
        f |= VmaFlags::ANONYMOUS;
    }
    f
}

#[inline]
fn validate_prot(prot: u32) -> Result<(), MmapError> {
    if prot & !KNOWN_PROT != 0 {
        return Err(MmapError::InvalidAddress);
    }
    if prot & PROT_WRITE != 0 && prot & PROT_EXEC != 0 {
        return Err(MmapError::PermissionDenied);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// do_mmap
// ─────────────────────────────────────────────────────────────────────────────

/// Maps a memory region in the current user address space.
///
/// - `addr`:  desired address (0 = kernel chooses).
/// - `len`:   size in bytes (rounded up to multiple of PAGE_SIZE).
/// - `prot`:  PROT_READ | PROT_WRITE | PROT_EXEC.
/// - `flags`: MAP_ANON | MAP_PRIVATE | MAP_SHARED | MAP_FIXED.
/// - `_fd`:   file descriptor (ignored — only anonymous mappings are supported).
/// - `_off`:  offset in file (ignored).
///
/// Returns the starting virtual address of the region (as usize).
pub fn do_mmap(
    addr: u64,
    len: usize,
    prot: u32,
    flags: u32,
    _fd: i32,
    _off: u64,
) -> Result<usize, MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: the pointer is valid as long as this thread's syscall is being handled.
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

    // Alignment to PAGE_SIZE
    let len_aligned = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    user_as.stats.mmap_calls.fetch_add(1, Ordering::Relaxed);

    // Search for a free virtual gap
    let fixed = flags & MAP_FIXED != 0;
    let base = if fixed {
        if addr == 0 || !addr.is_multiple_of(PAGE_SIZE as u64) {
            return Err(MmapError::InvalidAddress);
        }
        VirtAddr::new(addr)
    } else {
        let hint = if addr != 0 {
            Some(VirtAddr::new(addr))
        } else {
            None
        };
        user_as
            .find_free_gap(len_aligned, hint)
            .ok_or(MmapError::OutOfVirtualMemory)?
    };

    let start = base;
    let end_raw = base
        .as_u64()
        .checked_add(len_aligned as u64)
        .ok_or(MmapError::OutOfVirtualMemory)?;
    // T4-002 FIX: Strict check of user bounds (never in kernel half)
    if base.as_u64() < PAGE_SIZE as u64 || end_raw > crate::memory::core::layout::USER_END.as_u64() {
        return Err(MmapError::InvalidAddress);
    }
    let end = VirtAddr::new(end_raw);

    let page_flags = prot_to_page_flags(prot);
    let vma_flags = prot_to_vma_flags(prot, flags) | VmaFlags::ANONYMOUS;

    // Allocate and insert VMA descriptor
    let vma = Box::new(VmaDescriptor::new(
        start,
        end,
        vma_flags,
        page_flags,
        VmaBacking::Anonymous,
    ));
    let vma_ptr = Box::into_raw(vma);

    // SAFETY: vma_ptr is valid, non-null, exclusive.
    let inserted = unsafe { user_as.insert_vma(vma_ptr) };
    if !inserted {
        // Free if insertion fails (e.g. overlap with MAP_FIXED)
        // SAFETY: we are the exclusive owners.
        let _ = unsafe { Box::from_raw(vma_ptr) };
        return Err(MmapError::InvalidAddress);
    }

    Ok(start.as_u64() as usize)
}

// ─────────────────────────────────────────────────────────────────────────────
// do_munmap
// ─────────────────────────────────────────────────────────────────────────────

/// Unmaps the virtual region starting at `addr`.
///
/// `addr` must be aligned to PAGE_SIZE.
/// Unmaps all PTEs present in the VMA and frees their physical frames.
///
/// V-04: Synchronous TLB shootdown (all CPUs) BEFORE free_page.
pub fn do_munmap(addr: u64, _len: usize) -> Result<(), MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: user_as_ptr is non-null (guaranteed by get_current_user_as) and lives for the duration of the syscall.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };
    do_munmap_in_as(user_as, addr)
}

/// Unmaps a VMA in an explicit address space.
pub fn do_munmap_in_as(user_as: &UserAddressSpace, addr: u64) -> Result<(), MmapError> {
    if addr == 0 || !addr.is_multiple_of(PAGE_SIZE as u64) {
        return Err(MmapError::InvalidAddress);
    }

    user_as.stats.munmap_calls.fetch_add(1, Ordering::Relaxed);

    match user_as.remove_vma(VirtAddr::new(addr)) {
        None => Err(MmapError::NotMapped),
        Some(vma_ptr) => {
            // Retrieve VMA bounds and backing before freeing the descriptor.
            let (vma_start, vma_end, vma_backing) = unsafe {
                let vma = &*vma_ptr;
                (vma.start, vma.end, vma.backing)
            };

            // Free VmaDescriptor allocated by do_mmap / do_brk.
            // SAFETY: the pointer is valid and we are the owners.
            let _ = unsafe { Box::from_raw(vma_ptr) };

            // V-04: process pages in batches to bound buffer size.
            //   1. Unmap PTEs (integrated local TLB flush via unmap_page).
            //   2. Synchronous TLB shootdown across all active CPUs.
            //   3. Free accumulated physical frames.
            // This separation guarantees no CPU can access a frame
            // after its release via a stale TLB entry (V-04).
            const BATCH: usize = 64; // 64 × 4 KiB = 256 KiB per batch
            let cpu_count = crate::arch::x86_64::acpi::madt::madt_cpu_count();

            let mut cursor = vma_start.as_u64();
            while cursor < vma_end.as_u64() {
                let mut frames = [crate::memory::core::types::Frame::containing(
                    crate::memory::core::types::PhysAddr::new(0),
                ); BATCH];
                let mut count = 0usize;

                // Phase 1: unmap up to BATCH pages (integrated local flush).
                while cursor < vma_end.as_u64() && count < BATCH {
                    // SAFETY: canonical user addresses.
                    if let Some(f) = unsafe { user_as.unmap_page(VirtAddr::new(cursor)) } {
                        frames[count] = f;
                        count += 1;
                    }
                    cursor += PAGE_SIZE as u64;
                }

                if count == 0 {
                    continue;
                }

                // Phase 2: synchronous TLB shootdown (all CPUs) — V-04.
                let batch_start = VirtAddr::new(
                    vma_start
                        .as_u64()
                        .max(cursor - (count as u64 * PAGE_SIZE as u64)),
                );
                let batch_end = VirtAddr::new(cursor);
                // SAFETY: canonical user range, called outside IRQ.
                unsafe {
                    crate::memory::virt::shootdown_sync(
                        crate::memory::virt::TlbFlushType::Range {
                            start: batch_start,
                            end: batch_end,
                        },
                        cpu_count,
                    );
                }

                // Phase 3: free frames (TLBs already invalidated everywhere).
                // T4-001 FIX: NEVER free Shared/Device/Direct VMA frames to buddy allocator.
                if vma_backing == VmaBacking::Anonymous {
                    for i in 0..count {
                        let remaining = crate::memory::cow::tracker::COW_TRACKER.dec(frames[i]);
                        if remaining == 0 || remaining == u32::MAX {
                            let _ = crate::memory::physical::allocator::buddy::free_page(frames[i]);
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// do_mprotect
// ─────────────────────────────────────────────────────────────────────────────

/// Modifies permissions of an existing memory region.
///
/// `addr` must be aligned to PAGE_SIZE and match the start of a VMA.
/// Updates PTEs already present in page table AND invalidates TLB.
pub fn do_mprotect(addr: u64, len: usize, prot: u32) -> Result<(), MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: user_as_ptr is non-null (guaranteed by get_current_user_as) and lives for the duration of the syscall.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };
    do_mprotect_in_as(user_as, addr, len, prot)
}

/// Modifies permissions of a region in an explicit address space.
pub fn do_mprotect_in_as(
    user_as: &UserAddressSpace,
    addr: u64,
    len: usize,
    prot: u32,
) -> Result<(), MmapError> {
    if addr == 0 || !addr.is_multiple_of(PAGE_SIZE as u64) {
        return Err(MmapError::InvalidAddress);
    }
    if len == 0 {
        return Err(MmapError::InvalidLength);
    }
    validate_prot(prot)?;

    let vma_const_ptr = user_as
        .find_vma(VirtAddr::new(addr))
        .ok_or(MmapError::NotMapped)?;

    // SAFETY: The VMA is valid and protected by the address space lock.
    let vma: &mut VmaDescriptor = unsafe { &mut *(vma_const_ptr as *mut VmaDescriptor) };

    let len_aligned = (len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let range_start = VirtAddr::new(addr);
    let range_end_raw = addr
        .checked_add(len_aligned as u64)
        .ok_or(MmapError::InvalidLength)?;
    let range_end = VirtAddr::new(range_end_raw);
    if range_end.as_u64() > vma.end.as_u64() {
        return Err(MmapError::NotMapped);
    }

    let new_page_flags = prot_to_page_flags(prot);

    // Update page flags in VMA (for future demand-faults).
    vma.page_flags = new_page_flags;

    // Update VMA flags (READ/WRITE/EXEC only).
    let perm_mask = VmaFlags::READ | VmaFlags::WRITE | VmaFlags::EXEC;
    let keep = VmaFlags::from_bits(vma.flags.bits() & !perm_mask.bits());
    let mut new_perm = VmaFlags::NONE;
    if prot & PROT_READ != 0 {
        new_perm |= VmaFlags::READ;
    }
    if prot & PROT_WRITE != 0 {
        new_perm |= VmaFlags::WRITE;
    }
    if prot & PROT_EXEC != 0 {
        new_perm |= VmaFlags::EXEC;
    }
    vma.flags = keep | new_perm;

    // Apply new flags to PTEs already present in the table.
    // Pages not yet faulted (demand paging) receive an Err which is ignored:
    // they will inherit the new flags on their next fault via vma.page_flags.
    let mut walker = crate::memory::virt::page_table::PageTableWalker::new(user_as.pml4_phys());
    let mut cursor = range_start.as_u64();
    while cursor < range_end.as_u64() {
        let va = VirtAddr::new(cursor);
        let old_entry =
            crate::memory::virt::page_table::PageTableEntry::from_raw(walker.read_pte_raw(va));
        let is_shared_cow = old_entry.is_cow()
            || old_entry.frame().is_some_and(|f| {
                crate::memory::cow::tracker::COW_TRACKER
                    .tracked_ref_count(f)
                    .is_some_and(|rc| rc > 1)
            });
        let pte_flags = if is_shared_cow && new_page_flags.contains(PageFlags::WRITABLE) {
            new_page_flags
                .clear(PageFlags::WRITABLE)
                .set(PageFlags::COW)
        } else {
            new_page_flags
        };
        // remap_flags → Err if page absent (demand paging) → intentionally ignored.
        let _ = walker.remap_flags(va, pte_flags);
        cursor += PAGE_SIZE as u64;
    }

    // Invalidate TLB for the entire modified range, including if target AS
    // runs on another CPU.
    let cpu_count = crate::arch::x86_64::acpi::madt::madt_cpu_count();
    // SAFETY: canonical user addresses in [range_start, range_end), outside IRQ.
    unsafe {
        crate::memory::virt::shootdown_sync(
            crate::memory::virt::TlbFlushType::Range {
                start: range_start,
                end: range_end,
            },
            cpu_count,
        );
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// do_mremap_zero_copy
// ─────────────────────────────────────────────────────────────────────────────

/// Moves a complete VMA by moving PTEs, without copying data.
///
/// This path covers the critical allocator case: an entire private anonymous
/// region is expanded/relocated with `MREMAP_MAYMOVE`. Partial cases or
/// `MREMAP_DONTUNMAP` remain handled by the compatibility fallback on the
/// syscall side, as they require separate splitting/aliasing semantics.
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

    if old_addr == 0 || !old_addr.is_multiple_of(PAGE_SIZE as u64) {
        return Err(MmapError::InvalidAddress);
    }
    if old_len == 0 || new_len == 0 {
        return Err(MmapError::InvalidLength);
    }
    if flags & MREMAP_DONTUNMAP != 0 {
        return Ok(MremapZeroCopy::Unsupported);
    }
    if flags & MREMAP_MAYMOVE == 0 {
        return Ok(MremapZeroCopy::Unsupported);
    }
    if flags & MREMAP_FIXED != 0 && (new_addr == 0 || !new_addr.is_multiple_of(PAGE_SIZE as u64)) {
        return Err(MmapError::InvalidAddress);
    }

    let old_len_aligned = (old_len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let new_len_aligned = (new_len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    if new_len_aligned < old_len_aligned {
        return Ok(MremapZeroCopy::Unsupported);
    }
    let old_start = VirtAddr::new(old_addr);

    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: the pointer is valid as long as this thread's syscall is being handled.
    let user_as: &UserAddressSpace = unsafe { &*user_as_ptr };

    let old_vma_ptr = user_as.find_vma(old_start).ok_or(MmapError::NotMapped)?;
    // SAFETY: VMA pointer from current AS; instantaneous metadata read.
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
    let target_end = VirtAddr::new(target_end_raw);

    let mut moved_vma = old_vma.clone_metadata();
    moved_vma.start = target_start;
    moved_vma.end = target_end;

    let new_vma = Box::new(moved_vma);
    let new_vma_ptr = Box::into_raw(new_vma);
    // SAFETY: new_vma_ptr is a fresh allocation not yet attached.
    if !unsafe { user_as.insert_vma(new_vma_ptr) } {
        // SAFETY: insertion failed, so the tree did not take ownership.
        let _ = unsafe { Box::from_raw(new_vma_ptr) };
        return Err(MmapError::InvalidAddress);
    }

    struct PtAllocOnly;
    impl crate::memory::virt::page_table::FrameAllocatorForWalk for PtAllocOnly {
        fn alloc_frame(
            &self,
            flags: crate::memory::AllocFlags,
        ) -> Result<crate::memory::Frame, crate::memory::AllocError> {
            crate::memory::physical::allocator::buddy::alloc_pages(0, flags)
        }

        fn free_frame(&self, frame: crate::memory::Frame) {
            let _ = crate::memory::physical::allocator::buddy::free_pages(frame, 0);
        }
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
    // SAFETY: canonical user ranges, outside IRQ.
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

// ─────────────────────────────────────────────────────────────────────────────
// do_brk
// ─────────────────────────────────────────────────────────────────────────────

/// Historical fallback for spaces without a published ELF image. Normal
/// processes use `UserAddressSpace::heap_start`, initialized by the loader.
const BRK_BASE: u64 = 0x0000_0001_0000_0000; // 4 GiB

fn align_up_page(addr: u64) -> Option<u64> {
    let mask = PAGE_SIZE as u64 - 1;
    addr.checked_add(mask).map(|value| value & !mask)
}

/// Expands or shrinks user heap (per-process via `UserAddressSpace`).
///
/// - `addr == 0`: returns current break.
/// - `addr != 0`: moves break to `addr` (rounded up to next multiple
///   of PAGE_SIZE) and only creates the genuinely new VMA portion.
///
/// Returns the new break value.
pub fn do_brk(addr: u64) -> Result<u64, MmapError> {
    let user_as_ptr = get_current_user_as().ok_or(MmapError::NoAddressSpace)?;
    // SAFETY: user_as_ptr is non-null (guaranteed by get_current_user_as) and lives for the duration of the syscall.
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

        // SAFETY: vma_ptr is valid, non-null, exclusive — coming from Box::into_raw.
        let inserted = unsafe { user_as.insert_vma(vma_ptr) };
        if !inserted {
            // SAFETY: vma_ptr is still exclusive — insert_vma failed, reclaim ownership.
            let _ = unsafe { Box::from_raw(vma_ptr) };
            return Err(MmapError::OutOfVirtualMemory);
        }
    }
    // Shrink: VMAs are not removed (Linux behavior on shrink).

    // Update per-process break (no global state).
    user_as.heap_end.store(new_brk, Ordering::Release);
    Ok(new_brk)
}

// ─────────────────────────────────────────────────────────────────────────────
// map_shm_into_process  —  P1-02
// ─────────────────────────────────────────────────────────────────────────────

/// Errors specific to SHM mapping in an address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShmMapError {
    /// No target address space available.
    NoAddressSpace,
    /// Out of virtual memory in target space.
    OutOfVirtualMemory,
    /// Frame allocation or mapping failure.
    AllocFailed,
    /// Invalid or inactive SHM region.
    InvalidRegion,
    /// Permission denied (empty intersection).
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

/// IPC callbacks necessary to map an SHM region without importing IPC into memory/.
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
        release_region: unsafe { core::mem::transmute::<*mut (), ShmReleaseRegionFn>(release_region) },
        register_mapping: unsafe { core::mem::transmute::<*mut (), ShmRegisterMappingFn>(register_mapping) },
    })
}

/// Result of an SHM mapping into an address space.
pub struct ShmMapIntoResult {
    /// Base virtual address of the mapping in the target space.
    pub virt_base: u64,
    /// Number of pages actually mapped.
    pub n_pages: usize,
    /// Mapping index in SHM_MAPPING_TABLE (for shm_unmap).
    pub mapping_idx: usize,
}

fn free_removed_vma(user_as: &UserAddressSpace, start: VirtAddr) {
    if let Some(vma_ptr) = user_as.remove_vma(start) {
        // SAFETY: remove_vma() transfers descriptor ownership to the caller.
        let _ = unsafe { Box::from_raw(vma_ptr) };
    }
}

fn rollback_shm_into_process(user_as: &UserAddressSpace, virt_base: VirtAddr, n_pages: usize) {
    for i in 0..n_pages {
        let v = VirtAddr::new(virt_base.as_u64().saturating_add((i * PAGE_SIZE) as u64));
        unsafe {
            user_as.unmap_page(v);
        }
    }
    free_removed_vma(user_as, virt_base);
}

/// Maps an existing SHM region into a process's address space.
///
/// ## Operations
/// 1. Finds a free virtual gap in `user_as` (or uses `hint_virt`)
/// 2. Registers a VMA of type `VmaBacking::Shared`
/// 3. Maps each SHM physical frame via `user_as.map_page()` (NO_COW)
/// 4. Registers the mapping via the SHM provider injected by the IPC layer
///
/// ## Safety
/// `user_as` must be a valid pointer to an active `UserAddressSpace`.
/// SHM frames are provided by the registered provider and are never
/// freed while at least one mapping remains active.
///
/// ## Arguments
/// - `user_as`:   address space of the target process
/// - `desc_idx`:  opaque index of the SHM region on provider side
/// - `pid`:       process PID (for VMM hooks)
/// - `hint_virt`: desired virtual address (0 = auto)
/// - `writable`:  allow writing to the mapping
pub fn map_shm_into_process(
    user_as: &UserAddressSpace,
    desc_idx: usize,
    pid: u32,
    hint_virt: u64,
    writable: bool,
) -> Result<ShmMapIntoResult, ShmMapError> {
    use crate::memory::core::{Frame, PageFlags};
    use crate::memory::physical::allocator::buddy;

    let provider = shm_provider().ok_or(ShmMapError::InvalidRegion)?;

    // ── 1. Read SHM region metadata ───────────────────────────────────────
    let info = (provider.region_info)(desc_idx, writable)?;
    let n_pages = info.n_pages;
    let size_bytes = info.size_bytes;

    if n_pages == 0 {
        (provider.release_region)(desc_idx);
        return Err(ShmMapError::InvalidRegion);
    }

    let size_aligned = (size_bytes + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // ── 2. Find a free virtual gap ────────────────────────────────────────
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

    // ── 3. Build VMA flags ────────────────────────────────────────────────
    let vma_flags = if writable {
        VmaFlags::READ | VmaFlags::WRITE | VmaFlags::SHARED
    } else {
        VmaFlags::READ | VmaFlags::SHARED
    };
    // NO_COW on SHM pages (shared, no copy-on-write)
    let page_flags = if writable {
        PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::USER | PageFlags::NO_EXECUTE
    } else {
        PageFlags::PRESENT | PageFlags::USER | PageFlags::NO_EXECUTE
    };

    // ── 4. Insert VMA ─────────────────────────────────────────────────────
    let vma = Box::new(VmaDescriptor::new(
        virt_base,
        virt_end,
        vma_flags,
        page_flags,
        VmaBacking::Shared,
    ));
    let vma_ptr = Box::into_raw(vma);
    // SAFETY: vma_ptr is valid, non-null, exclusive.
    let inserted = unsafe { user_as.insert_vma(vma_ptr) };
    if !inserted {
        // SAFETY: reclaim the ownership we had.
        let _ = unsafe { Box::from_raw(vma_ptr) };
        (provider.release_region)(desc_idx);
        return Err(ShmMapError::OutOfVirtualMemory);
    }

    // ── 5. Map SHM physical frames ────────────────────────────────────────
    // Allocator for intermediate tables (PT/PD/PDPT) only —
    // data frames come from the SHM pool.
    struct PtAllocOnly;
    impl crate::memory::virt::page_table::FrameAllocatorForWalk for PtAllocOnly {
        fn alloc_frame(
            &self,
            flags: crate::memory::AllocFlags,
        ) -> Result<Frame, crate::memory::AllocError> {
            buddy::alloc_pages(0, flags)
        }
        fn free_frame(&self, f: Frame) {
            let _ = buddy::free_pages(f, 0);
        }
    }
    let pt_alloc = PtAllocOnly;

    {
        for i in 0..n_pages {
            let Some(phys) = (provider.page_phys)(desc_idx, i) else {
                rollback_shm_into_process(user_as, virt_base, i);
                (provider.release_region)(desc_idx);
                return Err(ShmMapError::InvalidRegion);
            };
            let frame = Frame::containing(crate::memory::core::PhysAddr::new(phys));
            let virt = VirtAddr::new(virt_base.as_u64().saturating_add((i * PAGE_SIZE) as u64));
            // SAFETY: virt is in target process's user space,
            // frame is an allocated and initialized SHM page.
            if unsafe { user_as.map_page(virt, frame, page_flags, &pt_alloc) }.is_err() {
                rollback_shm_into_process(user_as, virt_base, i);
                (provider.release_region)(desc_idx);
                return Err(ShmMapError::AllocFailed);
            }
        }
    }

    // ── 6. Register in SHM_MAPPING_TABLE ──────────────────────────────────
    let mapping_idx = (provider.register_mapping)(desc_idx, pid, virt_base.as_u64(), writable, n_pages)
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
