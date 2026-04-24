//! # arch/x86_64/boot — Initialisation au démarrage
//!
//! Gère le démarrage du kernel depuis le bootloader (Multiboot2 ou UEFI)
//! et la transition vers l'init du kernel principal.
//!
//! ## Séquence BSP
//! 1. `early_init` : configurer paging identité, GDT temporaire, IDT, TSS, APIC TSC
//! 2. `multiboot2::parse` ou `uefi::parse` : lire la memory map et les tags bootloader
//! 3. `acpi::init` : localiser MADT, HPET, FADT
//! 4. `smp::boot_aps` : démarrer les APs
//! 5. Appeler `kernel_main()`

pub mod early_init;
pub mod memory_map;
pub mod multiboot2;
pub mod trampoline_asm;
pub mod uefi;

pub use early_init::arch_boot_init;
pub use memory_map::{
    init_memory_subsystem_exoboot, init_memory_subsystem_multiboot2, init_memory_subsystem_uefi,
    MemoryRegion, MemoryRegionType, EXOBOOT_BOOT_INFO_MAGIC, EXOBOOT_MAGIC_U32, MEMORY_MAP,
    MEMORY_REGION_COUNT, PHYS_MEMORY_MAX, PHYS_MEMORY_START,
};
pub use multiboot2::{parse_multiboot2, Multiboot2Info};
pub use uefi::{parse_uefi_memmap, UefiMemoryMap};
