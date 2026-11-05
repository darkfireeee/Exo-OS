// kernel/src/memory/virtual/fault/demand_paging.rs
//
// Demand paging — allocates and maps a page on first access.
//   - Anonymous: guaranteed zero-fill (fresh frame from buddy + ZEROED flag)
//   - File:      delegates to registered FileFaultProvider (Layer 0 trait)
//   - Device:    maps the physical page supplied by DeviceFaultProvider
//   - Shared:    zero-fill with COW marker if shareable
//   - Direct:    already mapped, should never reach here
//
// Layer 0 — no scheduler/process/ipc/fs dependencies.

use core::sync::atomic::{AtomicU64, Ordering};

use super::handler::FaultAllocator;
use super::{FaultContext, FaultResult};
use crate::memory::core::{Frame, PageFlags, VirtAddr, PAGE_SIZE};
use crate::memory::virt::vma::{VmaBacking, VmaDescriptor, VmaFlags};

// ─────────────────────────────────────────────────────────────────────────────
// DEMAND PAGING STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct DemandPagingStats {
    pub anon_faults: AtomicU64,
    pub file_faults: AtomicU64,
    pub device_faults: AtomicU64,
    pub shared_faults: AtomicU64,
    pub oom_count: AtomicU64,
    pub map_errors: AtomicU64,
    /// Total pages allocated as zero-fill.
    pub zero_fill_pages: AtomicU64,
    /// Pages supplied by FileFaultProvider.
    pub file_filled_pages: AtomicU64,
    /// Pages supplied by DeviceFaultProvider.
    pub device_pages: AtomicU64,
}

impl Default for DemandPagingStats {
    fn default() -> Self {
        Self::new()
    }
}

impl DemandPagingStats {
    pub const fn new() -> Self {
        DemandPagingStats {
            anon_faults: AtomicU64::new(0),
            file_faults: AtomicU64::new(0),
            device_faults: AtomicU64::new(0),
            shared_faults: AtomicU64::new(0),
            oom_count: AtomicU64::new(0),
            map_errors: AtomicU64::new(0),
            zero_fill_pages: AtomicU64::new(0),
            file_filled_pages: AtomicU64::new(0),
            device_pages: AtomicU64::new(0),
        }
    }
}

pub static DEMAND_PAGING_STATS: DemandPagingStats = DemandPagingStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// FileFaultProvider TRAIT (Layer 0 — dependency injection)
// ─────────────────────────────────────────────────────────────────────────────

/// Content provider for file-backed pages.
///
/// Implemented by the VFS/page-cache layer outside Layer 0.
/// Registered via `register_file_fault_provider()`.
///
/// Contract:
///   - `file_id`:     opaque file identifier (supplied by `VmaDescriptor.file_id`)
///   - `file_offset`: offset in file (in bytes, PAGE_SIZE aligned)
///   - `dest_frame`:  destination physical frame, already allocated
///
/// The implementation must fill exactly PAGE_SIZE bytes into `dest_frame`.
/// Returns `Ok(())` if content was loaded, `Err(_)` otherwise.
pub trait FileFaultProvider: Sync {
    fn load_file_page(
        &self,
        file_id: u64,
        file_offset: u64,
        dest_frame: Frame,
    ) -> Result<(), crate::memory::core::AllocError>;
}

/// Frame provider for device-backed pages.
///
/// Registered via `register_device_fault_provider()`.
///
/// The implementation returns the physical frame corresponding to
/// (device_id, offset) without allocating new physical memory.
pub trait DeviceFaultProvider: Sync {
    fn get_device_page(&self, device_id: u64, offset: u64) -> Option<Frame>;
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL REGISTRIES (atomic pointer, Layer 0 safe)
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::AtomicPtr;

/// Globally registered file-backed page provider.
/// Initially null (Layer 0 works without it — zero-fill fallback).
static FILE_FAULT_PROVIDER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
/// Vtable stored separately (fat pointer).
static FILE_FAULT_VTABLE: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Device-backed page provider.
static DEVICE_FAULT_PROVIDER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static DEVICE_FAULT_VTABLE: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Registers a file-backed page provider.
///
/// Must be called once from the VFS layer (after memory init).
///
/// # Safety
/// Single-threaded call during VFS init.
pub unsafe fn register_file_fault_provider(data_ptr: *const (), vtable: *const ()) {
    FILE_FAULT_PROVIDER.store(data_ptr as *mut (), Ordering::Release);
    FILE_FAULT_VTABLE.store(vtable as *mut (), Ordering::Release);
}

/// Registers a device-backed frame provider.
///
/// # Safety
/// Single-threaded call during drivers init.
pub unsafe fn register_device_fault_provider(data_ptr: *const (), vtable: *const ()) {
    DEVICE_FAULT_PROVIDER.store(data_ptr as *mut (), Ordering::Release);
    DEVICE_FAULT_VTABLE.store(vtable as *mut (), Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// INTERNAL HELPER: allocate + map zero page
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
fn alloc_zero_map<A: FaultAllocator>(
    page_addr: VirtAddr,
    flags: PageFlags,
    alloc: &A,
) -> FaultResult {
    let frame = match alloc.alloc_zeroed() {
        Ok(f) => f,
        Err(_) => {
            DEMAND_PAGING_STATS
                .oom_count
                .fetch_add(1, Ordering::Relaxed);
            return FaultResult::Oom { addr: page_addr };
        }
    };
    DEMAND_PAGING_STATS
        .zero_fill_pages
        .fetch_add(1, Ordering::Relaxed);
    match alloc.map_page(page_addr, frame, flags) {
        Ok(_) => FaultResult::Handled,
        Err(_) => {
            alloc.free_frame(frame);
            DEMAND_PAGING_STATS
                .map_errors
                .fetch_add(1, Ordering::Relaxed);
            FaultResult::Oom { addr: page_addr }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MAIN HANDLER
// ─────────────────────────────────────────────────────────────────────────────

/// Handles a demand paging fault (page not present, legitimate access on valid VMA).
///
/// Dispatches according to `vma.backing`:
/// - `Anonymous`: guaranteed zero-fill
/// - `File`:      read via FileFaultProvider (fallback zero-fill if not registered)
/// - `Device`:    map physical frame supplied by DeviceFaultProvider
/// - `Shared`:    zero-fill + COW flags for future sharing
/// - `Direct`:    error (should not reach here)
pub fn handle_demand_paging<A: FaultAllocator>(
    ctx: &FaultContext,
    vma: &VmaDescriptor,
    alloc: &A,
) -> FaultResult {
    let page_addr = VirtAddr::new(ctx.fault_addr.as_u64() & !(PAGE_SIZE as u64 - 1));

    match vma.backing {
        // ── ANONYMOUS: zero-fill ─────────────────────────────────────────────
        VmaBacking::Anonymous => {
            DEMAND_PAGING_STATS
                .anon_faults
                .fetch_add(1, Ordering::Relaxed);
            let result = alloc_zero_map(page_addr, vma.page_flags, alloc);
            if result.is_handled() {
                vma.record_fault();
            }
            result
        }

        // ── FILE-BACKED: FileFaultProvider or zero-fill ──────────────────────
        VmaBacking::File => {
            DEMAND_PAGING_STATS
                .file_faults
                .fetch_add(1, Ordering::Relaxed);

            let data_ptr = FILE_FAULT_PROVIDER.load(Ordering::Acquire);
            let vtable = FILE_FAULT_VTABLE.load(Ordering::Acquire);

            if !data_ptr.is_null() && !vtable.is_null() {
                // Registered provider — allocate empty frame and fill it.
                let frame = match alloc.alloc_nonzeroed() {
                    Ok(f) => f,
                    Err(_) => {
                        DEMAND_PAGING_STATS
                            .oom_count
                            .fetch_add(1, Ordering::Relaxed);
                        return FaultResult::Oom {
                            addr: ctx.fault_addr,
                        };
                    }
                };

                // Compute file offset from the beginning of the VMA.
                let vma_offset = page_addr.as_u64().saturating_sub(vma.start.as_u64());
                let file_offset = vma.file_offset.saturating_add(vma_offset);

                // Reconstruct fat pointer and call provider.
                // SAFETY: valid ptrs, registered by register_file_fault_provider.
                let fat: (*const (), *const ()) = (data_ptr as *const (), vtable as *const ());
                let provider: &dyn FileFaultProvider = unsafe { core::mem::transmute(fat) };
                match provider.load_file_page(vma.inode_id, file_offset, frame) {
                    Ok(()) => {
                        DEMAND_PAGING_STATS
                            .file_filled_pages
                            .fetch_add(1, Ordering::Relaxed);
                        match alloc.map_page(page_addr, frame, vma.page_flags) {
                            Ok(_) => {
                                vma.record_fault();
                                FaultResult::Handled
                            }
                            Err(_) => {
                                alloc.free_frame(frame);
                                DEMAND_PAGING_STATS
                                    .map_errors
                                    .fetch_add(1, Ordering::Relaxed);
                                FaultResult::Oom {
                                    addr: ctx.fault_addr,
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // I/O error: never silently replace file-backed
                        // code or data with a zero page.
                        alloc.free_frame(frame);
                        FaultResult::Segfault {
                            addr: ctx.fault_addr,
                        }
                    }
                }
            } else {
                // No provider — zero-fill (acceptable during boot).
                let result = alloc_zero_map(page_addr, vma.page_flags, alloc);
                if result.is_handled() {
                    vma.record_fault();
                }
                result
            }
        }

        // ── DEVICE: frame provided by DeviceFaultProvider ───────────────────
        VmaBacking::Device => {
            DEMAND_PAGING_STATS
                .device_faults
                .fetch_add(1, Ordering::Relaxed);

            let data_ptr = DEVICE_FAULT_PROVIDER.load(Ordering::Acquire);
            let vtable = DEVICE_FAULT_VTABLE.load(Ordering::Acquire);

            if !data_ptr.is_null() && !vtable.is_null() {
                let vma_offset = page_addr.as_u64().saturating_sub(vma.start.as_u64());
                let device_offset = vma.file_offset.saturating_add(vma_offset);

                let fat: (*const (), *const ()) = (data_ptr as *const (), vtable as *const ());
                let provider: &dyn DeviceFaultProvider = unsafe { core::mem::transmute(fat) };
                if let Some(dev_frame) = provider.get_device_page(vma.inode_id, device_offset) {
                    // Map with device flags (no-cache, no-exec, present).
                    let dev_flags = vma.page_flags | PageFlags::PRESENT | PageFlags::NO_CACHE;
                    DEMAND_PAGING_STATS
                        .device_pages
                        .fetch_add(1, Ordering::Relaxed);
                    match alloc.map_page(page_addr, dev_frame, dev_flags) {
                        Ok(_) => {
                            vma.record_fault();
                            FaultResult::Handled
                        }
                        Err(_) => {
                            DEMAND_PAGING_STATS
                                .map_errors
                                .fetch_add(1, Ordering::Relaxed);
                            FaultResult::Oom {
                                addr: ctx.fault_addr,
                            }
                        }
                    }
                } else {
                    // Device does not provide this frame → SEGFAULT.
                    FaultResult::Segfault {
                        addr: ctx.fault_addr,
                    }
                }
            } else {
                // No device provider → zero-fill (incomplete config).
                let result = alloc_zero_map(page_addr, vma.page_flags, alloc);
                if result.is_handled() {
                    vma.record_fault();
                }
                result
            }
        }

        // ── SHARED: zero-fill + COW for sharing ─────────────────────────────
        VmaBacking::Shared => {
            DEMAND_PAGING_STATS
                .shared_faults
                .fetch_add(1, Ordering::Relaxed);
            // Shared anonymous: zero-fill, but flags must remain COW
            // so copying is triggered if any sharer writes.
            let shared_flags = if vma.flags.contains(VmaFlags::WRITE) {
                vma.page_flags | PageFlags::COW
            } else {
                vma.page_flags
            };
            let result = alloc_zero_map(page_addr, shared_flags, alloc);
            if result.is_handled() {
                vma.record_fault();
            }
            result
        }

        // ── DIRECT: already mapped, should not reach here ───────────────────
        VmaBacking::Direct => {
            // Direct physical mapping should have been established upon VMA
            // creation (mmap with MAP_FIXED + physaddr). A fault here is
            // an inconsistency: return SEGFAULT.
            FaultResult::Segfault {
                addr: ctx.fault_addr,
            }
        }
    }
}
