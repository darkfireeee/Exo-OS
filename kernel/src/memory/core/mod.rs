// kernel/src/memory/core/mod.rs
//
// Module core — types et constantes fondamentaux de gestion mémoire.
// Couche 0 — aucune dépendance externe au kernel.

pub mod address;
pub mod constants;
pub mod layout;
pub mod types;

// ─── Re-exports publics ───────────────────────────────────────────────────────

pub use constants::{
    BUDDY_MAX_BYTES, BUDDY_MAX_ORDER, BUDDY_MAX_PAGES, BUDDY_ORDER_COUNT, CACHE_LINE_SHIFT,
    CACHE_LINE_SIZE, DMA_RING_MASK, DMA_RING_SIZE, EMERGENCY_POOL_SIZE, FUTEX_HASH_BUCKETS,
    FUTEX_HASH_MASK, GIGA_PAGE_SHIFT, GIGA_PAGE_SIZE, HUGE_PAGE_MASK, HUGE_PAGE_SHIFT,
    HUGE_PAGE_SIZE, MAX_CPUS, MAX_NUMA_NODES, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE,
    PER_CPU_DRAIN_THRESHOLD, PER_CPU_POOL_SIZE, PER_CPU_REFILL_THRESHOLD, SLAB_MAX_OBJ_SIZE,
    SLAB_MIN_OBJ_SIZE, SLAB_NR_SIZE_CLASSES, STACK_CANARY_INITIAL, TLB_SHOOTDOWN_TIMEOUT_CYCLES,
    ZONE_DMA32_END, ZONE_DMA_END, ZONE_NORMAL_START,
};

pub use types::{
    AllocError, AllocFlags, Frame, FrameRange, Page, PageFlags, PageRange, PhysAddr, VirtAddr,
    ZoneType,
};

pub use address::{
    align_down, align_up, assert_invariants as assert_address_invariants, bytes_to_pages,
    canonicalize, frame_containing, huge_pages_for, in_kernel_heap, is_aligned, is_canonical,
    is_kernel_canonical, is_power_of_two, is_user_canonical, kernel_phys_to_virt,
    kernel_virt_to_phys, page_align_down, page_align_up, page_containing, pages_for,
    pages_to_bytes, pfn_to_phys, phys_to_pfn, phys_to_virt, virt_to_phys_physmap, PhysRange,
    VirtRange,
};

pub use layout::{
    fixmap_slot_addr, DMA_MAP_BASE, DMA_MAP_END, DMA_MAP_SIZE, FIXMAP_ACPI_0, FIXMAP_ACPI_1,
    FIXMAP_BASE, FIXMAP_END, FIXMAP_HPET, FIXMAP_IOAPIC, FIXMAP_LAPIC, FIXMAP_NR_RESERVED,
    FIXMAP_SIZE, FIXMAP_TEMP_MAP, IPC_RING_MAP_BASE, IPC_RING_MAP_END, IPC_RING_MAP_SIZE,
    KERNEL_HEAP_END, KERNEL_HEAP_SIZE, KERNEL_HEAP_START, KERNEL_IMAGE_END, KERNEL_IMAGE_MAX_SIZE,
    KERNEL_LOAD_PHYS_ADDR, KERNEL_PHYS_OFFSET, KERNEL_START, MODULES_BASE, MODULES_END,
    MODULES_SIZE, PHYS_MAP_BASE, PHYS_MAP_END, PHYS_MAP_SIZE, USER_ADDR_SPACE_SIZE, USER_END,
    USER_MMAP_BASE, USER_STACK_BASE, USER_STACK_DEFAULT_SIZE, USER_STACK_TOP, USER_START,
    VMALLOC_BASE, VMALLOC_END, VMALLOC_SIZE,
};
