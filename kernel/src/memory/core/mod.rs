// kernel/src/memory/core/mod.rs
//
// Module core — types et constantes fondamentaux de gestion mémoire.
// Couche 0 — aucune dépendance externe au kernel.

pub mod constants;
pub mod types;
pub mod address;
pub mod layout;

// ─── Re-exports publics ───────────────────────────────────────────────────────

pub use constants::{
    PAGE_SIZE, PAGE_SHIFT, PAGE_MASK,
    HUGE_PAGE_SIZE, HUGE_PAGE_SHIFT, HUGE_PAGE_MASK,
    GIGA_PAGE_SIZE, GIGA_PAGE_SHIFT,
    CACHE_LINE_SIZE, CACHE_LINE_SHIFT,
    BUDDY_MAX_ORDER, BUDDY_ORDER_COUNT, BUDDY_MAX_PAGES, BUDDY_MAX_BYTES,
    SLAB_NR_SIZE_CLASSES, SLAB_MIN_OBJ_SIZE, SLAB_MAX_OBJ_SIZE,
    PER_CPU_POOL_SIZE, PER_CPU_REFILL_THRESHOLD, PER_CPU_DRAIN_THRESHOLD,
    EMERGENCY_POOL_SIZE,
    TLB_SHOOTDOWN_TIMEOUT_CYCLES,
    MAX_CPUS, MAX_NUMA_NODES,
    ZONE_DMA_END, ZONE_DMA32_END, ZONE_NORMAL_START,
    DMA_RING_SIZE, DMA_RING_MASK,
    FUTEX_HASH_BUCKETS, FUTEX_HASH_MASK,
    STACK_CANARY_INITIAL,
};

pub use types::{
    PhysAddr, VirtAddr,
    Page, PageRange,
    Frame, FrameRange,
    PageFlags, ZoneType,
    AllocFlags, AllocError,
};

pub use address::{
    phys_to_virt, virt_to_phys_physmap,
    kernel_virt_to_phys, kernel_phys_to_virt,
    is_power_of_two, align_down, align_up, is_aligned,
    page_align_down, page_align_up,
    pages_for, huge_pages_for, bytes_to_pages, pages_to_bytes,
    frame_containing, page_containing,
    pfn_to_phys, phys_to_pfn,
    canonicalize, is_canonical, is_kernel_canonical, is_user_canonical,
    PhysRange, VirtRange,
    in_kernel_heap,
    assert_invariants as assert_address_invariants,
};

pub use layout::{
    PHYS_MAP_BASE, PHYS_MAP_SIZE, PHYS_MAP_END,
    VMALLOC_BASE, VMALLOC_SIZE, VMALLOC_END,
    MODULES_BASE, MODULES_SIZE, MODULES_END,
    FIXMAP_BASE, FIXMAP_SIZE, FIXMAP_END,
    KERNEL_HEAP_START, KERNEL_HEAP_SIZE, KERNEL_HEAP_END,
    IPC_RING_MAP_BASE, IPC_RING_MAP_SIZE, IPC_RING_MAP_END,
    DMA_MAP_BASE, DMA_MAP_SIZE, DMA_MAP_END,
    KERNEL_START, KERNEL_IMAGE_MAX_SIZE, KERNEL_IMAGE_END,
    KERNEL_LOAD_PHYS_ADDR, KERNEL_PHYS_OFFSET,
    USER_START, USER_END, USER_ADDR_SPACE_SIZE,
    USER_MMAP_BASE, USER_STACK_TOP, USER_STACK_DEFAULT_SIZE, USER_STACK_BASE,
    FIXMAP_LAPIC, FIXMAP_IOAPIC, FIXMAP_ACPI_0, FIXMAP_ACPI_1,
    FIXMAP_HPET, FIXMAP_TEMP_MAP, FIXMAP_NR_RESERVED,
    fixmap_slot_addr,
};
