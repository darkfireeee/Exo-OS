//! memory/ — Gestion mémoire bootloader (avant kernel/memory/).
//!
//! Ce module gère la mémoire PENDANT le démarrage, avant que le kernel
//! n'initialise son propre allocateur (kernel/src/memory/).
//!
//! DISTINCTION IMPORTANTE :
//!   memory/ (ce module) = BOOTLOADER uniquement
//!   kernel/src/memory/  = Gestion mémoire du kernel en Ring 0
//!   Ces deux modules sont INDÉPENDANTS (DOC10/BOOT-01).
//!
//! Responsabilités :
//!   1. `map.rs`     : Collecte carte mémoire (UEFI + E820) → format BootInfo unifié
//!   2. `paging.rs`  : Setup page tables initiales (identité + higher-half)
//!                     Le kernel reprendra avec ses propres page tables.
//!   3. `regions.rs` : Régions réservées (ACPI RSDP, MMIO, firmware)
//!
//! RÈGLE BOOT-04 : Chaque région mémoire doit avoir un type EXACT dans BootInfo.
//! RÈGLE BOOT-03 : BootInfo = contrat formel — aucun champ non initialisé.

pub mod map;
pub mod paging;
pub mod regions;

// ─── Re-exports ────────────────────────────────────────────────────────────────
pub use map::{MemoryMap, MemoryRegion, MemoryKind};
pub use paging::PageTablesSetup;

// ─── Constantes globales ───────────────────────────────────────────────────────

/// Taille d'une page (4 KB).
pub const PAGE_SIZE: usize = 4096;

/// Taille d'une huge page (2 MB).
pub const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Adresse de début du higher-half kernel (convention Exo-OS).
/// Le kernel est linké pour cette adresse virtuelle (PML4 index 511).
/// Synchronisé avec kernel/src/arch/x86_64/mod.rs::KERNEL_BASE.
pub const KERNEL_HIGHER_HALF_BASE: u64 = 0xFFFF_FFFF_8000_0000;

/// Adresse physique maximale supportée (512 GB — limite pratique x86_64 48-bit).
pub const MAX_PHYS_ADDR: u64 = 512 * 1024 * 1024 * 1024;

/// Nombre maximum de régions dans la MemoryMap du bootloader.
/// Assez large pour gérer des configurations complexes NUMA/MMIO.
pub const MAX_MEMORY_REGIONS: usize = 256;
