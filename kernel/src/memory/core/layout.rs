// kernel/src/memory/core/layout.rs
//
// Carte mémoire statique du noyau Exo-OS — x86_64.
// Toutes les constantes représentent des adresses virtuelles dans
// l'espace d'adressage noyau (au-dessus de 0xFFFF_8000_0000_0000).
// Couche 0 — aucune dépendance externe.

use super::constants::PAGE_SIZE;
use super::types::VirtAddr;

// ─────────────────────────────────────────────────────────────────────────────
// ESPACE D'ADRESSAGE x86_64 (4 niveaux, 48 bits)
//
//  0x0000_0000_0000_0000 - 0x0000_7FFF_FFFF_FFFF : Espace utilisateur (128 TiB)
//  0x0000_8000_0000_0000 - 0xFFFF_7FFF_FFFF_FFFF : Adresses non canoniques (trou)
//  0xFFFF_8000_0000_0000 - 0xFFFF_FFFF_FFFF_FFFF : Espace noyau (128 TiB)
//
// Layout noyau Exo-OS :
//
//  0xFFFF_8000_0000_0000 : PHYS_MAP_BASE     (physmap directe — 64 TiB)
//  0xFFFF_C000_0000_0000 : VMALLOC_BASE      (vmalloc — 32 TiB)
//  0xFFFF_E000_0000_0000 : VMPLAT_BASE       (modules / drivers — 8 TiB)
//  0xFFFF_E800_0000_0000 : FIXMAP_BASE       (fixmaps — 1 TiB)
//  0xFFFF_F000_0000_0000 : KERNEL_HEAP_START (heap noyau dynamique — 1 TiB)
//  0xFFFF_F100_0000_0000 : IPC_RING_MAP_BASE (rings IPC kernel → user — 256 GiB)
//  0xFFFF_F200_0000_0000 : DMA_MAP_BASE      (mapping DMA coherent — 256 GiB)
//  0xFFFF_FFFF_8000_0000 : KERNEL_START      (image noyau — 2 GiB max)
//  0xFFFF_FFFF_FFFF_FFF0 : haut de la stack noyau initiale
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// PHYSMAP — mapping direct de toute la RAM physique en noyau
// ─────────────────────────────────────────────────────────────────────────────

/// Base de la physmap : début de la région noyau.
/// Toute la RAM physique est accessible à PHYS_MAP_BASE + phys_addr.
pub const PHYS_MAP_BASE: VirtAddr = VirtAddr::new(0xFFFF_8000_0000_0000);

/// Taille de la physmap (64 TiB — couvre les systèmes NUMA les plus grands).
pub const PHYS_MAP_SIZE: usize = 64 * 1024 * 1024 * 1024 * 1024; // 64 TiB

/// Fin de la physmap (exclusive).
pub const PHYS_MAP_END: VirtAddr = VirtAddr::new(0xFFFF_C000_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// VMALLOC — allocations virtuellement contiguës (non contiguës physiquement)
// ─────────────────────────────────────────────────────────────────────────────

/// Base de la région vmalloc.
pub const VMALLOC_BASE: VirtAddr = VirtAddr::new(0xFFFF_C000_0000_0000);

/// Taille de la région vmalloc (32 TiB).
pub const VMALLOC_SIZE: usize = 32 * 1024 * 1024 * 1024 * 1024; // 32 TiB

/// Fin de la région vmalloc (exclusive).
pub const VMALLOC_END: VirtAddr = VirtAddr::new(0xFFFF_E000_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// MODULES / PLATFORM — espace pour les drivers et modules dynamiques
// ─────────────────────────────────────────────────────────────────────────────

/// Base des modules/drivers (après vmalloc).
pub const MODULES_BASE: VirtAddr = VirtAddr::new(0xFFFF_E000_0000_0000);

/// Taille de la région modules (8 TiB).
pub const MODULES_SIZE: usize = 8 * 1024 * 1024 * 1024 * 1024; // 8 TiB

/// Fin de la région modules (exclusive).
pub const MODULES_END: VirtAddr = VirtAddr::new(0xFFFF_E800_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// FIXMAP — mappings permanents à adresses fixes (APIC, ACPI...)
// ─────────────────────────────────────────────────────────────────────────────

/// Base de la région fixmap.
pub const FIXMAP_BASE: VirtAddr = VirtAddr::new(0xFFFF_E800_0000_0000);

/// Taille de la fixmap (1 TiB).
pub const FIXMAP_SIZE: usize = 1024 * 1024 * 1024 * 1024usize; // 1 TiB

/// Fin de la fixmap (exclusive).
pub const FIXMAP_END: VirtAddr = VirtAddr::new(0xFFFF_F000_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// HEAP NOYAU — allocations dynamiques du noyau (kmalloc/vmalloc)
// ─────────────────────────────────────────────────────────────────────────────

/// Base du heap noyau dynamique.
pub const KERNEL_HEAP_START: VirtAddr = VirtAddr::new(0xFFFF_F000_0000_0000);

/// Taille maximale du heap noyau (1 TiB).
pub const KERNEL_HEAP_SIZE: usize = 1024 * 1024 * 1024 * 1024usize; // 1 TiB

/// Fin du heap noyau (exclusive).
pub const KERNEL_HEAP_END: VirtAddr = VirtAddr::new(0xFFFF_F100_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// IPC RING MAP — buffers IPC partagés kernel↔user
// ─────────────────────────────────────────────────────────────────────────────

/// Base de la région IPC ring (en noyau, côté kernel).
pub const IPC_RING_MAP_BASE: VirtAddr = VirtAddr::new(0xFFFF_F100_0000_0000);

/// Taille de la région IPC ring (256 GiB).
pub const IPC_RING_MAP_SIZE: usize = 256 * 1024 * 1024 * 1024; // 256 GiB

/// Fin de la région IPC ring (exclusive).
pub const IPC_RING_MAP_END: VirtAddr = VirtAddr::new(0xFFFF_F200_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// DMA MAP — buffers DMA mappés en cohérence (UC ou WC selon type)
// ─────────────────────────────────────────────────────────────────────────────

/// Base de la région DMA map coherent.
pub const DMA_MAP_BASE: VirtAddr = VirtAddr::new(0xFFFF_F200_0000_0000);

/// Taille de la région DMA map (256 GiB).
pub const DMA_MAP_SIZE: usize = 256 * 1024 * 1024 * 1024; // 256 GiB

/// Fin de la région DMA map (exclusive).
pub const DMA_MAP_END: VirtAddr = VirtAddr::new(0xFFFF_F300_0000_0000);

// ─────────────────────────────────────────────────────────────────────────────
// IMAGE NOYAU — texte, données, BSS, rodata du noyau ELF linké
// ─────────────────────────────────────────────────────────────────────────────

/// Adresse virtuelle de LOAD de l'image noyau (link address).
pub const KERNEL_START: VirtAddr = VirtAddr::new(0xFFFF_FFFF_8000_0000);

/// Taille réservée pour l'image noyau (2 GiB max).
pub const KERNEL_IMAGE_MAX_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2 GiB

/// Fin réservée pour l'image noyau.
pub const KERNEL_IMAGE_END: VirtAddr = VirtAddr::new(0xFFFF_FFFF_FFFF_F000);

/// Offset physique d'où l'image noyau est chargée par le bootloader.
/// L'image est chargée à l'adresse physique 0x100000 (1 MiB) par GRUB.
pub const KERNEL_LOAD_PHYS_ADDR: u64 = 0x0010_0000; // 1 MiB

/// Offset de translation KERNEL_START → adresse physique.
/// virt = KERNEL_START + phys - KERNEL_LOAD_PHYS_ADDR
/// phys = virt - KERNEL_START + KERNEL_LOAD_PHYS_ADDR
pub const KERNEL_PHYS_OFFSET: VirtAddr =
    VirtAddr::new(KERNEL_START.as_u64() - KERNEL_LOAD_PHYS_ADDR);

// ─────────────────────────────────────────────────────────────────────────────
// ESPACE UTILISATEUR — limites de l'espace d'adressage user
// ─────────────────────────────────────────────────────────────────────────────

/// Première adresse valide dans l'espace utilisateur (évite la page nulle).
pub const USER_START: VirtAddr = VirtAddr::new(0x0000_0000_0001_0000);

/// Première adresse invalide après l'espace utilisateur canonique.
/// SAFETY: Cette valeur n'est PAS canonique (bit 47 = 1 → sign-extend vers noyau),
/// donc on doit utiliser new_unchecked pour préserver la valeur brute.
pub const USER_END: VirtAddr = unsafe { VirtAddr::new_unchecked(0x0000_8000_0000_0000) };

/// Taille maximale de l'espace d'adressage utilisateur (128 TiB - 64 KiB guard).
pub const USER_ADDR_SPACE_SIZE: usize =
    (0x0000_8000_0000_0000u64 - 0x0000_0000_0001_0000u64) as usize;

/// Adresse de base par défaut pour la première mmap() utilisateur.
pub const USER_MMAP_BASE: VirtAddr = VirtAddr::new(0x0000_0001_0000_0000); // 4 GiB

/// Stack utilisateur — sommet (s'étend vers le bas depuis USER_STACK_TOP).
pub const USER_STACK_TOP: VirtAddr = VirtAddr::new(0x0000_7FFF_FFFF_0000);

/// Taille maximale de la stack utilisateur (8 MiB par défaut, configurable via setrlimit).
pub const USER_STACK_DEFAULT_SIZE: usize = 8 * 1024 * 1024; // 8 MiB

/// Base de la stack utilisateur (USER_STACK_TOP - USER_STACK_DEFAULT_SIZE).
pub const USER_STACK_BASE: VirtAddr =
    VirtAddr::new(0x0000_7FFF_FFFF_0000 - USER_STACK_DEFAULT_SIZE as u64);

// ─────────────────────────────────────────────────────────────────────────────
// FIXMAP SLOTS — index prédéfinis dans la région fixmap
// ─────────────────────────────────────────────────────────────────────────────

/// Index dans la fixmap pour le LAPIC local.
pub const FIXMAP_LAPIC: usize = 0;

/// Index dans la fixmap pour l'I/O APIC.
pub const FIXMAP_IOAPIC: usize = 1;

/// Index dans la fixmap pour les tables ACPI.
pub const FIXMAP_ACPI_0: usize = 2;
pub const FIXMAP_ACPI_1: usize = 3;

/// Index dans la fixmap pour le HPET.
pub const FIXMAP_HPET: usize = 4;

/// Index dans la fixmap pour le mapping temporaire (utilisé par page table builder).
pub const FIXMAP_TEMP_MAP: usize = 5;

/// Nombre total de slots fixmap réservés au système.
pub const FIXMAP_NR_RESERVED: usize = 16;

/// Retourne l'adresse virtuelle d'un slot fixmap par son index.
#[inline(always)]
pub const fn fixmap_slot_addr(idx: usize) -> VirtAddr {
    // Les slots sont à FIXMAP_BASE + idx * PAGE_SIZE depuis la fin (croissant vers le bas)
    // Convention : les fixmaps s'adressent depuis la FIN de la région vers le bas
    VirtAddr::new(FIXMAP_END.as_u64() - (idx as u64 + 1) * PAGE_SIZE as u64)
}

// ─────────────────────────────────────────────────────────────────────────────
// VÉRIFICATIONS STATIQUES DE COHÉRENCE DU LAYOUT
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
