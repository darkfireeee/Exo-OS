// kernel/src/memory/core/layout.rs
//
// Static memory map of the Exo-OS kernel — x86_64.
// All constants represent virtual addresses in the
// kernel address space (above 0xFFFF_8000_0000_0000).
// Layer 0 — no external dependencies.

use core::sync::atomic::{AtomicU64, Ordering};

use super::constants::{GIGA_PAGE_SIZE, PAGE_SIZE};
use super::types::VirtAddr;

// ─────────────────────────────────────────────────────────────────────────────
// x86_64 ADDRESS SPACE (4 levels, 48 bits)
//
//  0x0000_0000_0000_0000 - 0x0000_7FFF_FFFF_FFFF : User space (128 TiB)
//  0x0000_8000_0000_0000 - 0xFFFF_7FFF_FFFF_FFFF : Non-canonical addresses (hole)
//  0xFFFF_8000_0000_0000 - 0xFFFF_FFFF_FFFF_FFFF : Kernel space (128 TiB)
//
// Exo-OS kernel layout:
//
//  0xFFFF_8000_0000_0000 : PHYS_MAP_BASE     (direct physmap — 64 TiB)
//  0xFFFF_C000_0000_0000 : VMALLOC_BASE      (vmalloc — 32 TiB)
//  0xFFFF_E000_0000_0000 : VMPLAT_BASE       (modules / drivers — 8 TiB)
//  0xFFFF_E800_0000_0000 : FIXMAP_BASE       (fixmaps — 1 TiB)
//  0xFFFF_F000_0000_0000 : KERNEL_HEAP_START (dynamic kernel heap — 1 TiB)
//  0xFFFF_F100_0000_0000 : IPC_RING_MAP_BASE (IPC rings kernel -> user — 256 GiB)
//  0xFFFF_F200_0000_0000 : DMA_MAP_BASE      (coherent DMA mapping — 256 GiB)
//  0xFFFF_FFFF_8000_0000 : KERNEL_START      (kernel image — 2 GiB max)
//  0xFFFF_FFFF_FFFF_FFF0 : top of initial kernel stack
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// PHYSMAP — direct mapping of all physical RAM into kernel
// ─────────────────────────────────────────────────────────────────────────────

/// Physmap base: start of kernel region.
/// All physical RAM is accessible at PHYS_MAP_BASE + phys_addr.
pub const PHYS_MAP_BASE: VirtAddr = VirtAddr::new(0xFFFF_8000_0000_0000);

/// Physmap size (64 TiB — covers largest NUMA systems).
pub const PHYS_MAP_SIZE: usize = 64 * 1024 * 1024 * 1024 * 1024; // 64 TiB

/// Initial coverage guaranteed by boot tables prior to extension.
pub const PHYSMAP_INITIAL_COVERAGE: usize = GIGA_PAGE_SIZE; // 1 GiB

static PHYSMAP_LIMIT: AtomicU64 = AtomicU64::new(PHYSMAP_INITIAL_COVERAGE as u64);

/// End of physmap (exclusive).
pub const PHYS_MAP_END: VirtAddr = VirtAddr::new(0xFFFF_C000_0000_0000);

#[inline]
pub fn physmap_limit() -> u64 {
    PHYSMAP_LIMIT.load(Ordering::Acquire)
}

#[inline]
pub fn set_physmap_limit(limit: u64) {
    let capped = limit.min(PHYS_MAP_SIZE as u64);
    let mut current = PHYSMAP_LIMIT.load(Ordering::Acquire);
    while capped > current {
        match PHYSMAP_LIMIT.compare_exchange(current, capped, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// VMALLOC — virtually contiguous allocations (physically non-contiguous)
// ─────────────────────────────────────────────────────────────────────────────

/// Base of vmalloc region.
pub const VMALLOC_BASE: VirtAddr = VirtAddr::new(0xFFFF_C000_0000_0000);

/// Size of vmalloc region (32 TiB).
pub const VMALLOC_SIZE: usize = 32 * 1024 * 1024 * 1024 * 1024; // 32 TiB

/// End of vmalloc region (exclusive).
pub const VMALLOC_END: VirtAddr = VirtAddr::new(0xFFFF_E000_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// MODULES / PLATFORM — space for drivers and dynamic modules
// ─────────────────────────────────────────────────────────────────────────────

/// Base for modules/drivers (after vmalloc).
pub const MODULES_BASE: VirtAddr = VirtAddr::new(0xFFFF_E000_0000_0000);

/// Size of modules region (8 TiB).
pub const MODULES_SIZE: usize = 8 * 1024 * 1024 * 1024 * 1024; // 8 TiB

/// End of modules region (exclusive).
pub const MODULES_END: VirtAddr = VirtAddr::new(0xFFFF_E800_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// FIXMAP — permanent mappings at fixed addresses (APIC, ACPI...)
// ─────────────────────────────────────────────────────────────────────────────

/// Base of fixmap region.
pub const FIXMAP_BASE: VirtAddr = VirtAddr::new(0xFFFF_E800_0000_0000);

/// Size of fixmap (1 TiB).
pub const FIXMAP_SIZE: usize = 1024 * 1024 * 1024 * 1024usize; // 1 TiB

/// End of fixmap (exclusive).
pub const FIXMAP_END: VirtAddr = VirtAddr::new(0xFFFF_F000_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// KERNEL HEAP — dynamic kernel allocations (kmalloc/vmalloc)
// ─────────────────────────────────────────────────────────────────────────────

/// Base of dynamic kernel heap.
pub const KERNEL_HEAP_START: VirtAddr = VirtAddr::new(0xFFFF_F000_0000_0000);

/// Maximum kernel heap size (1 TiB).
pub const KERNEL_HEAP_SIZE: usize = 1024 * 1024 * 1024 * 1024usize; // 1 TiB

/// End of kernel heap (exclusive).
pub const KERNEL_HEAP_END: VirtAddr = VirtAddr::new(0xFFFF_F100_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// IPC RING MAP — shared kernel↔user IPC buffers
// ─────────────────────────────────────────────────────────────────────────────

/// Base of IPC ring region (in kernel, kernel side).
pub const IPC_RING_MAP_BASE: VirtAddr = VirtAddr::new(0xFFFF_F100_0000_0000);

/// Size of IPC ring region (256 GiB).
pub const IPC_RING_MAP_SIZE: usize = 256 * 1024 * 1024 * 1024; // 256 GiB

/// End of IPC ring region (exclusive).
pub const IPC_RING_MAP_END: VirtAddr = VirtAddr::new(0xFFFF_F200_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// DMA MAP — coherent mapped DMA buffers (UC or WC depending on type)
// ─────────────────────────────────────────────────────────────────────────────

/// Base of coherent DMA map region.
pub const DMA_MAP_BASE: VirtAddr = VirtAddr::new(0xFFFF_F200_0000_0000);

/// Size of DMA map region (256 GiB).
pub const DMA_MAP_SIZE: usize = 256 * 1024 * 1024 * 1024; // 256 GiB

/// End of DMA map region (exclusive).
pub const DMA_MAP_END: VirtAddr = VirtAddr::new(0xFFFF_F300_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// KERNEL IMAGE — text, data, BSS, rodata of linked ELF kernel
// ─────────────────────────────────────────────────────────────────────────────

/// Virtual LOAD address of kernel image (link address).
pub const KERNEL_START: VirtAddr = VirtAddr::new(0xFFFF_FFFF_8000_0000);

/// Reserved size for kernel image (2 GiB max).
pub const KERNEL_IMAGE_MAX_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2 GiB

/// Reserved end for kernel image.
pub const KERNEL_IMAGE_END: VirtAddr = VirtAddr::new(0xFFFF_FFFF_FFFF_F000);

/// Physical offset where kernel image is loaded by bootloader.
/// Image is loaded at physical address 0x100000 (1 MiB) by GRUB.
pub const KERNEL_LOAD_PHYS_ADDR: u64 = 0x0010_0000; // 1 MiB

/// Translation offset KERNEL_START -> physical address.
/// virt = KERNEL_START + phys - KERNEL_LOAD_PHYS_ADDR
/// phys = virt - KERNEL_START + KERNEL_LOAD_PHYS_ADDR
pub const KERNEL_PHYS_OFFSET: VirtAddr =
    VirtAddr::new(KERNEL_START.as_u64() - KERNEL_LOAD_PHYS_ADDR);

// ─────────────────────────────────────────────────────────────────────────────
// USER SPACE — limits of user address space
// ─────────────────────────────────────────────────────────────────────────────

/// First valid address in user space (avoids null page).
pub const USER_START: VirtAddr = VirtAddr::new(0x0000_0000_0001_0000);

/// First invalid address after canonical user space.
/// SAFETY: This value is NOT canonical (bit 47 = 1 -> sign-extend towards kernel),
/// therefore we must use new_unchecked to preserve the raw value.
pub const USER_END: VirtAddr = unsafe { VirtAddr::new_unchecked(0x0000_8000_0000_0000) };

/// Maximum user address space size (128 TiB - 64 KiB guard).
pub const USER_ADDR_SPACE_SIZE: usize =
    (0x0000_8000_0000_0000u64 - 0x0000_0000_0001_0000u64) as usize;

/// Default base address for initial user mmap().
pub const USER_MMAP_BASE: VirtAddr = VirtAddr::new(0x0000_0001_0000_0000); // 4 GiB

/// User stack — top (extends downwards from USER_STACK_TOP).
pub const USER_STACK_TOP: VirtAddr = VirtAddr::new(0x0000_7FFF_FFFF_0000);

/// Maximum user stack size (8 MiB by default, configurable via setrlimit).
pub const USER_STACK_DEFAULT_SIZE: usize = 8 * 1024 * 1024; // 8 MiB

/// Initial stack physically mapped at startup of an ELF image.
///
/// The rlimit remains `USER_STACK_DEFAULT_SIZE`; these pages are only
/// the minimal eager bootstrap prior to demand-paged growth.
pub const USER_STACK_BOOTSTRAP_PAGES: usize = 8;
pub const USER_STACK_BOOTSTRAP_SIZE: usize = USER_STACK_BOOTSTRAP_PAGES * PAGE_SIZE;

/// User stack base (USER_STACK_TOP - USER_STACK_DEFAULT_SIZE).
pub const USER_STACK_BASE: VirtAddr =
    VirtAddr::new(0x0000_7FFF_FFFF_0000 - USER_STACK_DEFAULT_SIZE as u64);

// ─────────────────────────────────────────────────────────────────────────────
// FIXMAP SLOTS — predefined indices in fixmap region
// ─────────────────────────────────────────────────────────────────────────────

/// Fixmap index for local LAPIC.
pub const FIXMAP_LAPIC: usize = 0;

/// Fixmap index for I/O APIC.
pub const FIXMAP_IOAPIC: usize = 1;

/// Fixmap index for ACPI tables.
pub const FIXMAP_ACPI_0: usize = 2;
pub const FIXMAP_ACPI_1: usize = 3;

/// Fixmap index for HPET.
pub const FIXMAP_HPET: usize = 4;

/// Fixmap index for temporary mapping (used by page table builder).
pub const FIXMAP_TEMP_MAP: usize = 5;

/// Total number of system-reserved fixmap slots.
pub const FIXMAP_NR_RESERVED: usize = 16;

/// Returns virtual address of a fixmap slot by its index.
#[inline(always)]
pub const fn fixmap_slot_addr(idx: usize) -> VirtAddr {
    // Slots are at FIXMAP_BASE + idx * PAGE_SIZE from the end (growing downwards)
    // Convention: fixmaps are addressed from the END of the region downwards
    VirtAddr::new(FIXMAP_END.as_u64() - (idx as u64 + 1) * PAGE_SIZE as u64)
}

// ─────────────────────────────────────────────────────────────────────────────
// STATIC LAYOUT CONSISTENCY CHECKS
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(
    PHYS_MAP_BASE.as_u64() >= 0xFFFF_8000_0000_0000,
    "PHYS_MAP_BASE doit être dans l'espace noyau"
);
const _: () = assert!(
    VMALLOC_BASE.as_u64() >= PHYS_MAP_END.as_u64(),
    "VMALLOC_BASE doit suivre la physmap"
);
const _: () = assert!(
    MODULES_BASE.as_u64() >= VMALLOC_END.as_u64(),
    "MODULES_BASE doit suivre vmalloc"
);
const _: () = assert!(
    FIXMAP_BASE.as_u64() >= MODULES_END.as_u64(),
    "FIXMAP_BASE doit suivre modules"
);
const _: () = assert!(
    KERNEL_HEAP_START.as_u64() >= FIXMAP_END.as_u64(),
    "KERNEL_HEAP_START doit suivre fixmap"
);
const _: () = assert!(
    DMA_MAP_BASE.as_u64() >= IPC_RING_MAP_END.as_u64(),
    "DMA_MAP_BASE doit suivre IPC_RING_MAP"
);
const _: () = assert!(
    KERNEL_START.as_u64() >= DMA_MAP_END.as_u64(),
    "KERNEL_START doit suivre DMA_MAP"
);
const _: () = assert!(
    USER_END.as_u64() < PHYS_MAP_BASE.as_u64(),
    "Espace utilisateur doit terminer avant l'espace noyau"
);
const _: () = assert!(
    USER_STACK_TOP.as_u64() < USER_END.as_u64(),
    "USER_STACK_TOP doit être dans l'espace utilisateur"
);
const _: () = assert!(
    KERNEL_IMAGE_MAX_SIZE <= 2 * 1024 * 1024 * 1024,
    "Image noyau ne peut dépasser 2 GiB"
);
