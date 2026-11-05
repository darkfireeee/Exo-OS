// kernel/src/memory/heap/large/vmalloc.rs
//
// Allocator for large kernel allocations (> 2048 bytes).
// Uses the space VMALLOC_BASE -> VMALLOC_BASE + VMALLOC_SIZE.
// Each allocation is prefixed by a VmallocHeader (64 bytes, cacheline-aligned).
//
// LAYER 0 — no dependencies on scheduler/ipc/fs/process.
// Only internal dependency: physical::allocator::buddy, core::layout, core::types.

use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::core::address::phys_to_virt;
use crate::memory::core::constants::PAGE_SIZE;
use crate::memory::core::layout::PHYS_MAP_BASE;
use crate::memory::core::types::{AllocError, AllocFlags, Frame, PhysAddr};
use crate::memory::core::BUDDY_MAX_ORDER;
use crate::memory::physical::allocator::buddy::{alloc_pages, free_pages};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// VMALLOC ALLOCATION HEADER
// ─────────────────────────────────────────────────────────────────────────────

/// Magic to detect corruption or double-free.
const VMALLOC_MAGIC: u64 = 0xDEAD_BEEF_CAFE_BABE;
/// Flag indicating header is valid (not freed).
const HEADER_ALIVE: u32 = 0xAB_CD_12_34;
/// Flag for a freed header.
const HEADER_FREED: u32 = 0xDE_AD_00_00;

/// Header preceding each vmalloc allocation.
/// Size: 64 bytes (one cache line), always aligned to 64.
#[repr(C, align(64))]
pub struct VmallocHeader {
    /// Anti-corruption magic.
    magic: u64,
    /// Requested size by caller (excluding header).
    user_size: usize,
    /// Actual number of allocated pages (includes header pages).
    page_count: usize,
    /// Requested alignment (power of 2).
    alignment: usize,
    /// State (HEADER_ALIVE / HEADER_FREED).
    state: u32,
    /// Buddy order of underlying physical allocation.
    buddy_order: u32,
    /// Physical address of first frame.
    phys_base: u64,
    /// Padding to reach exactly 64 bytes.
    _pad: [u8; 16],
}

const _: () = assert!(core::mem::size_of::<VmallocHeader>() == 64);
const _: () = assert!(core::mem::align_of::<VmallocHeader>() == 64);

const VMALLOC_REGISTRY_CAP: usize = 4096;

#[derive(Clone, Copy)]
struct VmallocRegistryEntry {
    user_addr: u64,
    header_addr: u64,
}

impl VmallocRegistryEntry {
    const EMPTY: Self = Self {
        user_addr: 0,
        header_addr: 0,
    };

    #[inline]
    fn is_empty(self) -> bool {
        self.user_addr == 0
    }
}

struct VmallocRegistry {
    entries: [VmallocRegistryEntry; VMALLOC_REGISTRY_CAP],
}

impl VmallocRegistry {
    const fn new() -> Self {
        Self {
            entries: [VmallocRegistryEntry::EMPTY; VMALLOC_REGISTRY_CAP],
        }
    }

    fn insert(&mut self, user_addr: u64, header_addr: u64) -> bool {
        let mut free_slot = None;
        for idx in 0..self.entries.len() {
            let entry = self.entries[idx];
            if entry.user_addr == user_addr {
                self.entries[idx].header_addr = header_addr;
                return true;
            }
            if entry.is_empty() && free_slot.is_none() {
                free_slot = Some(idx);
            }
        }
        if let Some(idx) = free_slot {
            self.entries[idx] = VmallocRegistryEntry {
                user_addr,
                header_addr,
            };
            return true;
        }
        false
    }

    fn lookup(&self, user_addr: u64) -> Option<u64> {
        for entry in &self.entries {
            if entry.user_addr == user_addr {
                return Some(entry.header_addr);
            }
        }
        None
    }

    fn remove(&mut self, user_addr: u64) -> Option<u64> {
        for entry in &mut self.entries {
            if entry.user_addr == user_addr {
                let header_addr = entry.header_addr;
                *entry = VmallocRegistryEntry::EMPTY;
                return Some(header_addr);
            }
        }
        None
    }
}

static VMALLOC_REGISTRY: Mutex<VmallocRegistry> = Mutex::new(VmallocRegistry::new());

// ─────────────────────────────────────────────────────────────────────────────
// INTERNAL VMALLOC ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Statistics for the vmalloc allocator.
pub struct VmallocStats {
    pub alloc_count: AtomicU64,
    pub free_count: AtomicU64,
    pub bytes_inuse: AtomicU64,
    pub pages_inuse: AtomicU64,
    pub oom_count: AtomicU64,
    pub double_free: AtomicU64,
    pub corruption: AtomicU64,
    pub large_allocs: AtomicU64, // size > 1 MiB
}

impl VmallocStats {
    const fn new() -> Self {
        VmallocStats {
            alloc_count: AtomicU64::new(0),
            free_count: AtomicU64::new(0),
            bytes_inuse: AtomicU64::new(0),
            pages_inuse: AtomicU64::new(0),
            oom_count: AtomicU64::new(0),
            double_free: AtomicU64::new(0),
            corruption: AtomicU64::new(0),
            large_allocs: AtomicU64::new(0),
        }
    }
}

pub static VMALLOC_STATS: VmallocStats = VmallocStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// PUBLIC API
// ─────────────────────────────────────────────────────────────────────────────

/// Calculates the number of pages required for `size` bytes + header.
#[inline]
fn pages_needed(size: usize, align: usize) -> usize {
    // Header occupies at least 64 bytes (one cache line / 1 page when align >= PAGE_SIZE).
    let total = size
        .saturating_add(core::mem::size_of::<VmallocHeader>())
        .saturating_add(align.saturating_sub(1));
    (total + PAGE_SIZE - 1) / PAGE_SIZE
}

/// Calculates the minimum buddy order to cover `pages` pages.
#[inline]
fn order_for_pages(pages: usize) -> u32 {
    let mut order = 0u32;
    let mut cap = 1usize;
    while cap < pages {
        cap <<= 1;
        order += 1;
    }
    order
}

#[inline]
fn header_metadata_valid(header: &VmallocHeader, header_addr: u64) -> bool {
    if header.magic != VMALLOC_MAGIC || header.state != HEADER_ALIVE {
        return false;
    }
    if header.user_size == 0
        || header.page_count == 0
        || header.alignment == 0
        || !header.alignment.is_power_of_two()
        || header.buddy_order as usize > BUDDY_MAX_ORDER
    {
        return false;
    }
    if header.phys_base & (PAGE_SIZE as u64 - 1) != 0 {
        return false;
    }
    if PHYS_MAP_BASE.as_u64().saturating_add(header.phys_base) != header_addr {
        return false;
    }
    let expected_pages = pages_needed(header.user_size, header.alignment);
    if expected_pages != header.page_count {
        return false;
    }
    let expected_order = order_for_pages(header.page_count);
    if expected_order != header.buddy_order {
        return false;
    }
    header.page_count <= (1usize << header.buddy_order)
}

/// Allocates `size` bytes with `alignment` (power of 2, minimum 8).
/// Allocation is bounded by full pages.
///
/// # Errors
/// - `AllocError::OutOfMemory` — no frames available.
/// - `AllocError::InvalidParams` — zero size or alignment not a power of 2.
pub fn kalloc(size: usize, flags: AllocFlags) -> Result<NonNull<u8>, AllocError> {
    if size == 0 {
        return Err(AllocError::InvalidParams);
    }

    let align = if flags.contains(AllocFlags::DMA) {
        PAGE_SIZE
    } else {
        8
    };
    let pages = pages_needed(size, align);
    let order = order_for_pages(pages);

    // Allocate physical frames via buddy.
    let frame = alloc_pages(order as usize, flags)?;
    let phys_base = frame.phys_addr();

    // Obtain virtual address via physmap.
    let virt_base = phys_to_virt(phys_base);

    // SAFETY: The physmap completely covers physical RAM.
    // `virt_base` is the start of the mapped region for these frames.
    let header_ptr = virt_base.as_u64() as *mut VmallocHeader;

    // Initialize header.
    // SAFETY: Region is freshly allocated, uninitialized -> safe to write.
    unsafe {
        header_ptr.write(VmallocHeader {
            magic: VMALLOC_MAGIC,
            user_size: size,
            page_count: pages,
            alignment: align,
            state: HEADER_ALIVE,
            buddy_order: order,
            phys_base: phys_base.as_u64(),
            _pad: [0u8; 16],
        });
    }

    // User object begins right after header.
    let user_ptr = virt_base.as_u64() + core::mem::size_of::<VmallocHeader>() as u64;

    if !VMALLOC_REGISTRY.lock().insert(user_ptr, virt_base.as_u64()) {
        let _ = free_pages(frame, order as usize);
        VMALLOC_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
        return Err(AllocError::OutOfMemory);
    }

    // Update statistics.
    VMALLOC_STATS.alloc_count.fetch_add(1, Ordering::Relaxed);
    VMALLOC_STATS
        .bytes_inuse
        .fetch_add(size as u64, Ordering::Relaxed);
    VMALLOC_STATS
        .pages_inuse
        .fetch_add(pages as u64, Ordering::Relaxed);
    if size > 1024 * 1024 {
        VMALLOC_STATS.large_allocs.fetch_add(1, Ordering::Relaxed);
    }

    // Zero-fill if requested.
    if flags.contains(AllocFlags::ZEROED) {
        // SAFETY: Entire allocated region is valid; user_ptr + size < end of region.
        unsafe {
            core::ptr::write_bytes(user_ptr as *mut u8, 0, size);
        }
    }

    // SAFETY: buddy returned a valid frame, user_ptr is non-null.
    Ok(unsafe { NonNull::new_unchecked(user_ptr as *mut u8) })
}

/// Frees an allocation created by `kalloc`.
///
/// # Safety
/// `ptr` must have been returned by `kalloc` and not already freed.
pub unsafe fn kfree(ptr: NonNull<u8>, _hint_size: usize) {
    // Find the header preceding the user area.
    let user_addr = ptr.as_ptr() as u64;
    if user_addr < core::mem::size_of::<VmallocHeader>() as u64 {
        VMALLOC_STATS.corruption.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let Some(header_addr) = VMALLOC_REGISTRY.lock().lookup(user_addr) else {
        VMALLOC_STATS.corruption.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let header_ptr = header_addr as *mut VmallocHeader;

    // SAFETY: The header is at expected address in physmap.
    let header = &mut *header_ptr;

    // Verify magic.
    if header.magic != VMALLOC_MAGIC {
        VMALLOC_STATS.corruption.fetch_add(1, Ordering::Relaxed);
        // Do not continue: corrupt heap, leave leak rather than panic.
        return;
    }

    // Verify state.
    if header.state == HEADER_FREED {
        VMALLOC_STATS.double_free.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if header.state != HEADER_ALIVE || !header_metadata_valid(header, header_addr) {
        VMALLOC_STATS.corruption.fetch_add(1, Ordering::Relaxed);
        return;
    }

    let user_size = header.user_size;
    let page_count = header.page_count;
    let buddy_order = header.buddy_order;
    let phys_base = PhysAddr::new(header.phys_base);

    // Mark as freed before calling buddy (memory fence).
    header.state = HEADER_FREED;
    core::sync::atomic::fence(Ordering::Release);
    let _ = VMALLOC_REGISTRY.lock().remove(user_addr);

    // Poison user zone (helps detect UAF).
    core::ptr::write_bytes(user_addr as *mut u8, 0xAB, user_size);

    // Free physical frames via buddy.
    if free_pages(Frame::from_phys_addr(phys_base), buddy_order as usize).is_err() {
        VMALLOC_STATS.corruption.fetch_add(1, Ordering::Relaxed);
        return;
    }

    // Update statistics.
    VMALLOC_STATS.free_count.fetch_add(1, Ordering::Relaxed);
    VMALLOC_STATS
        .bytes_inuse
        .fetch_sub(user_size as u64, Ordering::Relaxed);
    VMALLOC_STATS
        .pages_inuse
        .fetch_sub(page_count as u64, Ordering::Relaxed);
}

/// Reallocates `ptr` with a new size.
///
/// # Safety
/// `ptr` must be valid (returned by `kalloc`).
pub unsafe fn krealloc(
    ptr: NonNull<u8>,
    _old_size: usize,
    new_size: usize,
    flags: AllocFlags,
) -> Result<NonNull<u8>, AllocError> {
    if new_size == 0 {
        kfree(ptr, _old_size);
        return Err(AllocError::InvalidParams);
    }

    let user_addr = ptr.as_ptr() as u64;
    let header_addr = VMALLOC_REGISTRY
        .lock()
        .lookup(user_addr)
        .ok_or(AllocError::InvalidParams)?;
    let header = &mut *(header_addr as *mut VmallocHeader);
    if !header_metadata_valid(header, header_addr) {
        VMALLOC_STATS.corruption.fetch_add(1, Ordering::Relaxed);
        return Err(AllocError::InvalidParams);
    }
    let old_user_size = header.user_size;
    let old_pages = header.page_count;
    let new_pages = pages_needed(new_size, header.alignment);
    if old_pages == new_pages {
        // Update user_size in header.
        header.user_size = new_size;
        if new_size >= old_user_size {
            VMALLOC_STATS
                .bytes_inuse
                .fetch_add((new_size - old_user_size) as u64, Ordering::Relaxed);
        } else {
            VMALLOC_STATS
                .bytes_inuse
                .fetch_sub((old_user_size - new_size) as u64, Ordering::Relaxed);
        }
        return Ok(ptr);
    }

    // Allocate + copy + free.
    let new_ptr = kalloc(new_size, flags)?;
    let copy_len = old_user_size.min(new_size);
    core::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_ptr(), copy_len);
    kfree(ptr, old_user_size);
    Ok(new_ptr)
}

/// Returns user size of allocation pointed to by `ptr`,
/// or `None` if header is corrupt.
///
/// # Safety
/// `ptr` must be an address returned by `kalloc`.
pub unsafe fn kalloc_usable_size(ptr: NonNull<u8>) -> Option<usize> {
    let user_addr = ptr.as_ptr() as u64;
    if user_addr < core::mem::size_of::<VmallocHeader>() as u64 {
        return None;
    }
    let header_addr = VMALLOC_REGISTRY.lock().lookup(user_addr)?;
    let header = &*(header_addr as *const VmallocHeader);
    if !header_metadata_valid(header, header_addr) {
        return None;
    }
    Some(header.user_size)
}
