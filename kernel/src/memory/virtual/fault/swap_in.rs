// kernel/src/memory/virtual/fault/swap_in.rs
//
// Swap-in fault handler — reloads a page from swap storage.
//
// Swap PTE format (Exo-OS, x86_64, PRESENT=0, PTE!=0):
//   Bits [63:12] = disk block number (swap block number)
//   Bits [11:8]  = swap device index (0..15)
//   Bits  [7:1]  = reserved (zero)
//   Bit   [0]    = 0 (PRESENT = not present → this is a swap PTE)
//
//   Exo-OS encodes this value in the PTE when a page is swapped out.
//   The value 0 (completely zero PTE) means "never allocated".
//
// Layer 0 — no external dependency except `spin`.

use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use super::handler::FaultAllocator;
use super::{FaultContext, FaultResult};
use crate::memory::core::{AllocError, Frame, VirtAddr, PAGE_SIZE};
use crate::memory::swap::{SwapError, SwapSlot, SWAP_BACKEND};
use crate::memory::virt::vma::VmaDescriptor;

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT SwapInProvider (Layer 0 — dependency injection)
// ─────────────────────────────────────────────────────────────────────────────

/// Page provider from swap storage.
///
/// Implemented by the swap/ layer (non Layer 0).
/// Registered via `register_swap_provider()`.
///
/// # Contract
/// - `swap_device`: swap device index (bits [11:8] of the PTE)
/// - `swap_block`: block number within this device (bits [63:12])
/// - `dest_frame`: destination frame (already allocated, uninitialized)
///
/// The implementation must fill exactly PAGE_SIZE bytes into `dest_frame`.
pub trait SwapInProvider: Sync {
    fn read_swap_page(
        &self,
        swap_device: u8,
        swap_block: u64,
        dest_frame: Frame,
    ) -> Result<(), AllocError>;
}

struct BackendSwapInProvider;

#[repr(C)]
struct FatPtr {
    data: *const (),
    vtable: *const (),
}

impl SwapInProvider for BackendSwapInProvider {
    fn read_swap_page(
        &self,
        swap_device: u8,
        swap_block: u64,
        dest_frame: Frame,
    ) -> Result<(), AllocError> {
        unsafe {
            SWAP_BACKEND.read_page(
                swap_device as usize,
                SwapSlot(swap_block),
                dest_frame.start_address(),
            )
        }
        .map_err(map_swap_error)
    }
}

static BACKEND_SWAP_IN_PROVIDER: BackendSwapInProvider = BackendSwapInProvider;

fn map_swap_error(err: SwapError) -> AllocError {
    match err {
        SwapError::NoSlot | SwapError::DeviceFull => AllocError::OutOfMemory,
        SwapError::NoDevice | SwapError::InvalidSlot | SwapError::NotEnabled => {
            AllocError::InvalidParams
        }
        SwapError::IoError | SwapError::Corrupted => AllocError::WouldBlock,
    }
}

pub fn register_backend_swap_provider() {
    let provider: &'static dyn SwapInProvider = &BACKEND_SWAP_IN_PROVIDER;
    // SAFETY: representation [data, vtable] of fat pointers `&dyn Trait`
    // used only to serialize the provider into the global registry.
    let fat: FatPtr = unsafe { core::mem::transmute(provider) };
    // SAFETY: `provider` is static and remains valid for the entire lifetime of the kernel.
    unsafe {
        register_swap_provider(fat.data, fat.vtable);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GLOBAL PROVIDER REGISTRY
// ─────────────────────────────────────────────────────────────────────────────

/// Pointer to the active SwapInProvider (fat pointer — data + vtable).
static SWAP_PROVIDER_DATA: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SWAP_PROVIDER_VTABLE: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Registers the global swap-in provider.
///
/// Must be called once from the swap subsystem (after init).
///
/// # Safety
/// - `data_ptr` and `vtable` must form a valid fat pointer to
///   a `dyn SwapInProvider` object with `'static` lifetime.
/// - Single-threaded call.
pub unsafe fn register_swap_provider(data_ptr: *const (), vtable: *const ()) {
    SWAP_PROVIDER_DATA.store(data_ptr as *mut (), Ordering::Release);
    SWAP_PROVIDER_VTABLE.store(vtable as *mut (), Ordering::Release);
}

/// Unregisters the swap provider (for tests or hot-unplug).
pub fn unregister_swap_provider() {
    SWAP_PROVIDER_DATA.store(core::ptr::null_mut(), Ordering::Release);
    SWAP_PROVIDER_VTABLE.store(core::ptr::null_mut(), Ordering::Release);
}

/// Returns `true` if a swap provider is registered.
#[inline]
pub fn swap_provider_present() -> bool {
    !SWAP_PROVIDER_DATA.load(Ordering::Acquire).is_null()
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTICS
// ─────────────────────────────────────────────────────────────────────────────

pub struct SwapInStats {
    /// Number of swap-in faults received.
    pub total: AtomicU64,
    /// Pages successfully reloaded from swap.
    pub success: AtomicU64,
    /// Faults where no provider was registered → zero-fill fallback.
    pub no_provider: AtomicU64,
    /// Provider I/O errors (failed swap read).
    pub io_errors: AtomicU64,
    /// OOM during destination frame allocation.
    pub oom_count: AtomicU64,
    /// Mapping errors after successful read.
    pub map_errors: AtomicU64,
    /// Null PTEs (page never swapped, demand paging fallback).
    pub null_pte: AtomicU64,
}

impl Default for SwapInStats {
    fn default() -> Self {
        Self::new()
    }
}

impl SwapInStats {
    pub const fn new() -> Self {
        SwapInStats {
            total: AtomicU64::new(0),
            success: AtomicU64::new(0),
            no_provider: AtomicU64::new(0),
            io_errors: AtomicU64::new(0),
            oom_count: AtomicU64::new(0),
            map_errors: AtomicU64::new(0),
            null_pte: AtomicU64::new(0),
        }
    }
}

pub static SWAP_IN_STATS: SwapInStats = SwapInStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// SWAP PTE DECODING
// ─────────────────────────────────────────────────────────────────────────────

/// Extracts the swap device index and block number from a raw PTE.
///
/// Exo-OS format:
///   device  = (pte_raw >> 8) & 0xF      (4 bits → 16 devices max)
///   block   = (pte_raw >> 12) & ((1<<52)-1)  (52 bits of block number)
/// Returns `None` if `pte_raw == 0` (PTE never written).
#[inline]
fn decode_swap_pte(pte_raw: u64) -> Option<(u8, u64)> {
    if pte_raw == 0 {
        return None;
    }
    let device = ((pte_raw >> 8) & 0xF) as u8;
    let block = (pte_raw >> 12) & ((1u64 << 52) - 1);
    Some((device, block))
}

// ─────────────────────────────────────────────────────────────────────────────
// MAIN HANDLER
// ─────────────────────────────────────────────────────────────────────────────

/// Handles a swap-in fault.
///
/// 1. Reads the raw value of the not-present PTE (encoded swap entry).
/// 2. If PTE == 0 → delegates to demand_paging (page never swapped).
/// 3. Decodes swap device and block number.
/// 4. Allocates a destination physical frame.
/// 5. Calls `SwapInProvider::read_swap_page()` to fill the frame.
/// 6. Maps the frame into the address space with the VMA flags.
pub fn handle_swap_in<A: FaultAllocator>(
    ctx: &FaultContext,
    vma: &VmaDescriptor,
    alloc: &A,
) -> FaultResult {
    SWAP_IN_STATS.total.fetch_add(1, Ordering::Relaxed);

    let page_addr = VirtAddr::new(ctx.fault_addr.as_u64() & !(PAGE_SIZE as u64 - 1));

    // Read the raw PTE for this page.
    let pte_raw = alloc.read_pte_raw(page_addr);

    // Null PTE → the page was never swapped: demand paging.
    let (swap_device, swap_block) = match decode_swap_pte(pte_raw) {
        Some(t) => t,
        None => {
            SWAP_IN_STATS.null_pte.fetch_add(1, Ordering::Relaxed);
            return super::demand_paging::handle_demand_paging(ctx, vma, alloc);
        }
    };

    // Retrieve the registered provider.
    let data_ptr = SWAP_PROVIDER_DATA.load(Ordering::Acquire);
    let vtable = SWAP_PROVIDER_VTABLE.load(Ordering::Acquire);

    if data_ptr.is_null() || vtable.is_null() {
        // No provider → zero-fill and continue (suboptimal but non-crashing).
        SWAP_IN_STATS.no_provider.fetch_add(1, Ordering::Relaxed);
        let frame = match alloc.alloc_zeroed() {
            Ok(f) => f,
            Err(_) => {
                SWAP_IN_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
                return FaultResult::Oom {
                    addr: ctx.fault_addr,
                };
            }
        };
        return match alloc.map_page(page_addr, frame, vma.page_flags) {
            Ok(_) => {
                vma.record_fault();
                FaultResult::Handled
            }
            Err(_) => {
                alloc.free_frame(frame);
                SWAP_IN_STATS.map_errors.fetch_add(1, Ordering::Relaxed);
                FaultResult::Oom {
                    addr: ctx.fault_addr,
                }
            }
        };
    }

    // Allocate the destination frame (non-zeroed, provider will fill it).
    let frame = match alloc.alloc_nonzeroed() {
        Ok(f) => f,
        Err(_) => {
            SWAP_IN_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
            return FaultResult::Oom {
                addr: ctx.fault_addr,
            };
        }
    };

    // Reconstruct the fat pointer and call read_swap_page.
    // SAFETY: fat pointer registered by `register_swap_provider`, 'static.
    let fat = FatPtr {
        data: data_ptr as *const (),
        vtable: vtable as *const (),
    };
    let provider: &dyn SwapInProvider = unsafe { core::mem::transmute(fat) };

    match provider.read_swap_page(swap_device, swap_block, frame) {
        Ok(()) => {
            // Map the reloaded page with VMA flags.
            match alloc.map_page(page_addr, frame, vma.page_flags) {
                Ok(_) => {
                    SWAP_IN_STATS.success.fetch_add(1, Ordering::Relaxed);
                    vma.record_fault();
                    FaultResult::Handled
                }
                Err(_) => {
                    alloc.free_frame(frame);
                    SWAP_IN_STATS.map_errors.fetch_add(1, Ordering::Relaxed);
                    FaultResult::Oom {
                        addr: ctx.fault_addr,
                    }
                }
            }
        }
        Err(_) => {
            // I/O error: free the frame and notify the caller.
            alloc.free_frame(frame);
            SWAP_IN_STATS.io_errors.fetch_add(1, Ordering::Relaxed);
            // Return OOM (the page will remain swapped, process will be retried or killed).
            FaultResult::Oom {
                addr: ctx.fault_addr,
            }
        }
    }
}
