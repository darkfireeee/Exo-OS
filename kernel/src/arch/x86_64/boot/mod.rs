//! # arch/x86_64/boot — Boot Initialization
//!
//! Handles kernel boot from bootloader (Multiboot2 or UEFI)
//! and the transition to main kernel initialization.
//!
//! ## BSP Sequence
//! 1. `early_init`: configure identity paging, temporary GDT, IDT, TSS, APIC TSC
//! 2. `multiboot2::parse` or `uefi::parse`: read memory map and bootloader tags
//! 3. `acpi::init`: locate MADT, HPET, FADT
//! 4. `smp::boot_aps`: boot APs
//! 5. Call `kernel_main()`

pub mod early_init;
pub mod memory_map;
// PATCH-P2-BOOT: multiboot2 module is DEPRECATED (Strata vision: UEFI-only).
// Kept for QEMU/dev compatibility. Active by default (default feature).
// Production UEFI-only: cargo build --no-default-features
// DEPRECATED - will be removed when Phase 8 (exo-boot UEFI GPT) is complete.
#[cfg_attr(not(feature = "multiboot2_compat"), allow(dead_code, unused_imports))]
pub mod multiboot2;
pub mod trampoline_asm;
pub mod uefi;

pub use early_init::arch_boot_init;
pub use memory_map::{
    init_memory_subsystem_exoboot, init_memory_subsystem_uefi, MemoryRegion, MemoryRegionType,
    EXOBOOT_BOOT_INFO_MAGIC, EXOBOOT_MAGIC_U32, MEMORY_MAP, MEMORY_REGION_COUNT, PHYS_MEMORY_MAX,
    PHYS_MEMORY_START,
};
// PATCH-P2-BOOT: exports multiboot2 gates.
// DEPRECATED - active by default (default feature). Disable: --no-default-features
#[cfg(feature = "multiboot2_compat")]
pub use memory_map::init_memory_subsystem_multiboot2;
#[cfg(feature = "multiboot2_compat")]
pub use multiboot2::{parse_multiboot2, Multiboot2Info};
pub use uefi::{parse_uefi_memmap, UefiMemoryMap};
