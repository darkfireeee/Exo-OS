//! # arch/x86_64/boot  Initialisation au demarrage
//!
//! Gere le demarrage du kernel depuis le bootloader (Multiboot2 ou UEFI)
//! et la transition vers l'init du kernel principal.
//!
//! ## Sequence BSP
//! 1. `early_init` : configurer paging identite, GDT temporaire, IDT, TSS, APIC TSC
//! 2. `multiboot2::parse` ou `uefi::parse` : lire la memory map et les tags bootloader
//! 3. `acpi::init` : localiser MADT, HPET, FADT
//! 4. `smp::boot_aps` : demarrer les APs
//! 5. Appeler `kernel_main()`

pub mod early_init;
pub mod memory_map;
// PATCH-P2-BOOT: le module multiboot2 est DEPRECIE (vision Strata : UEFI-only).
// Conserve pour compatibilite QEMU/dev. Par defaut actif (default feature).
// Production UEFI-only: cargo build --no-default-features
// DEPRECIE - sera retire quand Phase 8 (exo-boot UEFI GPT) sera complete.
#[cfg_attr(not(feature = "multiboot2_compat"),
    allow(dead_code, unused_imports))]
pub mod multiboot2;
pub mod trampoline_asm;
pub mod uefi;

pub use early_init::arch_boot_init;
pub use memory_map::{
    init_memory_subsystem_exoboot, init_memory_subsystem_uefi,
    MemoryRegion, MemoryRegionType, EXOBOOT_BOOT_INFO_MAGIC, EXOBOOT_MAGIC_U32, MEMORY_MAP,
    MEMORY_REGION_COUNT, PHYS_MEMORY_MAX, PHYS_MEMORY_START,
};
// PATCH-P2-BOOT: exports multiboot2 gates.
// DEPRECIE - actif par defaut (default feature). Desactiver: --no-default-features
#[cfg(feature = "multiboot2_compat")]
pub use memory_map::init_memory_subsystem_multiboot2;
#[cfg(feature = "multiboot2_compat")]
pub use multiboot2::{parse_multiboot2, Multiboot2Info};
pub use uefi::{parse_uefi_memmap, UefiMemoryMap};
