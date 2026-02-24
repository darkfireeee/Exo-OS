// kernel/src/arch/x86_64/boot/memory_map.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PONT CARTE MÉMOIRE BOOT ↔ MEMORY/ — x86_64
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module traduit la mémoire physique décrite par le bootloader (Multiboot2
// ou UEFI) en appels vers le sous-système memory/ pour démarrer les allocateurs.
//
// ## Séquence d'initialisation (règle MEM-02, DOC2)
//
//   1. EmergencyPool::init()           — PREMIER, avant tout allocateur
//   2. init_memory_subsystem_multiboot2() ou init_memory_subsystem_uefi()
//      a. init_phase1_bitmap(total_phys_start, total_phys_end)
//      b. Pour chaque région `Usable` de la E820 :
//         init_phase2_free_region(start, end)
//   3. init_phase3_slab_slub()         — après physmap
//   4. init_phase4_numa(nodes_mask)    — après topologie ACPI
//   5. KernelAddressSpace::init()      — PML4 kernel
//   6. memory_iface::init_memory_integration() — IPI TLB sender

#![allow(dead_code)]

use crate::arch::x86_64::boot::multiboot2::{MmapEntry, Multiboot2Info, MMAP_AVAILABLE};
use crate::arch::x86_64::boot::uefi::UefiMemoryMap;
use crate::memory::core::{PhysAddr, PAGE_SIZE};
use crate::memory::physical::allocator::{
    init_phase1_bitmap, init_phase2_free_region, init_phase3_slab_slub, init_phase4_numa,
};

// ─────────────────────────────────────────────────────────────────────────────
// LIMITES PHYSIQUES DU SYSTÈME
// ─────────────────────────────────────────────────────────────────────────────

/// Adresse physique de début de la RAM identifiée.
/// Réservée jusqu'à 1 MiB pour le legacy BIOS/ISA.
pub const PHYS_MEMORY_START: u64 = 0x0010_0000; // 1 MiB

/// Adresse physique maximale supportée (48 bits x86_64 PA).
pub const PHYS_MEMORY_MAX: u64 = (1u64 << 48) - 1;

// ─────────────────────────────────────────────────────────────────────────────
// CARTE MÉMOIRE RAPPORTÉE AU KERNEL
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de régions mémoire dans la carte E820 (couvre les cas réels).
pub const MAX_MEMORY_REGIONS: usize = 256;

/// Type de région mémoire (aligné sur E820 / UEFI map).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRegionType {
    /// RAM utilisable.
    Usable,
    /// Réservée (firmware, BIOS, DMA réservé).
    Reserved,
    /// ACPI reclaimable (peut être libérée après parse ACPI).
    AcpiReclaimable,
    /// ACPI NVS (non volatile storage — ne jamais libérer).
    AcpiNvs,
    /// Défectueuse (erreurs mémoire détectées par le firmware).
    Bad,
    /// Image du kernel (chargée par le bootloader).
    KernelImage,
}

/// Région de la carte mémoire physique.
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    pub base:        u64,
    pub size:        u64,
    pub region_type: MemoryRegionType,
}

impl MemoryRegion {
    #[inline]
    pub fn end(&self) -> u64 { self.base + self.size }

    #[inline]
    pub fn is_usable(&self) -> bool {
        matches!(self.region_type, MemoryRegionType::Usable)
    }
}

/// Carte mémoire statique (construite au boot, immutable ensuite).
pub static mut MEMORY_MAP: [MemoryRegion; MAX_MEMORY_REGIONS] = [
    MemoryRegion { base: 0, size: 0, region_type: MemoryRegionType::Reserved };
    MAX_MEMORY_REGIONS
];

/// Nombre de régions valides dans `MEMORY_MAP`.
pub static mut MEMORY_REGION_COUNT: usize = 0;

// ─────────────────────────────────────────────────────────────────────────────
// INIT DEPUIS MULTIBOOT2
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise le sous-système mémoire à partir d'une structure Multiboot2.
///
/// Lit les entrées MmapEntry et appelle les phases d'init du buddy allocator.
///
/// # Safety
/// - Doit être appelé UNE SEULE FOIS depuis le BSP, avant SMP.
/// - `info.mmap_ptr` doit pointer sur un tableau valide de `info.mmap_count` MmapEntry.
/// - EmergencyPool doit avoir été initialisé avant cet appel (RÈGLE MEM-02).
pub unsafe fn init_memory_subsystem_multiboot2(info: &Multiboot2Info) {
    debug_assert!(info.mmap_count > 0, "Multiboot2 : mmap vide");
    debug_assert!(info.mmap_ptr  != 0, "Multiboot2 : mmap_ptr nul");

    // ── Phase 1 : détecter la plage physique totale ───────────────────────────

    // SAFETY: mmap_ptr et mmap_count validés par le parseur multiboot2.
    let entries = core::slice::from_raw_parts(
        info.mmap_ptr as *const MmapEntry,
        info.mmap_count as usize,
    );

    let mut phys_start = u64::MAX;
    let mut phys_end   = 0u64;
    let mut region_count = 0usize;

    for entry in entries {
        let base = entry.base_addr;
        let end  = base + entry.length;

        // Filtrer les régions sous 1 MiB (legacy BIOS / ISA)
        if end <= PHYS_MEMORY_START { continue; }
        let base_adj = base.max(PHYS_MEMORY_START);

        if base_adj < phys_start { phys_start = base_adj; }
        if end > phys_end        { phys_end   = end; }

        // Remplir MEMORY_MAP
        if region_count < MAX_MEMORY_REGIONS {
            let rtype = e820_type_to_region_type(entry.entry_type);
            MEMORY_MAP[region_count] = MemoryRegion {
                base:        base_adj,
                size:        end.saturating_sub(base_adj),
                region_type: rtype,
            };
            region_count += 1;
        }
    }

    MEMORY_REGION_COUNT = region_count;

    if phys_start == u64::MAX || phys_end == 0 {
        // Aucune RAM trouvée — situation non récupérable
        crate::arch::x86_64::halt_cpu();
    }

    // Aligner sur des pages
    let phys_start_pa = PhysAddr::new(align_up(phys_start, PAGE_SIZE as u64));
    let phys_end_pa   = PhysAddr::new(align_down(phys_end, PAGE_SIZE as u64));

    // ── Phase 1 : initialiser le bitmap allocateur ────────────────────────────
    init_phase1_bitmap(phys_start_pa, phys_end_pa);

    // ── Phase 2 : libérer toutes les régions utilisables ─────────────────────
    for entry in entries {
        if entry.entry_type != MMAP_AVAILABLE { continue; }

        let base = entry.base_addr;
        let end  = base + entry.length;
        if end <= PHYS_MEMORY_START { continue; }

        let base_adj = align_up(base.max(PHYS_MEMORY_START), PAGE_SIZE as u64);
        let end_adj  = align_down(end.min(PHYS_MEMORY_MAX), PAGE_SIZE as u64);
        if base_adj >= end_adj { continue; }

        init_phase2_free_region(PhysAddr::new(base_adj), PhysAddr::new(end_adj));
    }

    // ── Phase 3 : Slab / SLUB ─────────────────────────────────────────────────
    init_phase3_slab_slub();

    // ── Phase 4 : NUMA (nœud 0 par défaut si pas encore de topologie ACPI) ───
    init_phase4_numa(0b0000_0001); // nœud 0 actif
}

/// Initialise le sous-système mémoire à partir d'une UEFI memory map.
///
/// Convertit les descripteurs EFI en régions mémoire et appelle les phases
/// d'init du buddy allocator.
///
/// # Safety
/// Mêmes contraintes que `init_memory_subsystem_multiboot2`.
pub unsafe fn init_memory_subsystem_uefi(uefi_map: &UefiMemoryMap) {
    let mut phys_start   = u64::MAX;
    let mut phys_end     = 0u64;
    let mut region_count = 0usize;

    // ── Première passe : détecter les bornes et remplir MEMORY_MAP ────────────
    for desc in uefi_map.iter() {
        let base = desc.physical_start;
        let end  = base + desc.number_of_pages * PAGE_SIZE as u64;

        if end <= PHYS_MEMORY_START { continue; }
        let base_adj = base.max(PHYS_MEMORY_START);

        if base_adj < phys_start { phys_start = base_adj; }
        if end > phys_end        { phys_end   = end; }

        if region_count < MAX_MEMORY_REGIONS {
            let rtype = uefi_type_to_region_type(desc.mem_type);
            MEMORY_MAP[region_count] = MemoryRegion {
                base:        base_adj,
                size:        end.saturating_sub(base_adj),
                region_type: rtype,
            };
            region_count += 1;
        }
    }

    MEMORY_REGION_COUNT = region_count;

    if phys_start == u64::MAX || phys_end == 0 {
        crate::arch::x86_64::halt_cpu();
    }

    let phys_start_pa = PhysAddr::new(align_up(phys_start, PAGE_SIZE as u64));
    let phys_end_pa   = PhysAddr::new(align_down(phys_end, PAGE_SIZE as u64));

    // ── Phase 1 : bitmap allocateur ──────────────────────────────────────────
    init_phase1_bitmap(phys_start_pa, phys_end_pa);

    // ── Phase 2 : libérer les régions conventionnelles ────────────────────────
    for desc in uefi_map.iter() {
        if !desc.is_usable() { continue; }

        let base = desc.physical_start;
        let end  = base + desc.number_of_pages * PAGE_SIZE as u64;
        if end <= PHYS_MEMORY_START { continue; }

        let base_adj = align_up(base.max(PHYS_MEMORY_START), PAGE_SIZE as u64);
        let end_adj  = align_down(end.min(PHYS_MEMORY_MAX), PAGE_SIZE as u64);
        if base_adj >= end_adj { continue; }

        init_phase2_free_region(PhysAddr::new(base_adj), PhysAddr::new(end_adj));
    }

    // ── Phase 3 & 4 ──────────────────────────────────────────────────────────
    init_phase3_slab_slub();
    init_phase4_numa(0b0000_0001);
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPERS PRIVÉS
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit un type E820 (Multiboot2) en MemoryRegionType.
#[inline]
fn e820_type_to_region_type(e820_type: u32) -> MemoryRegionType {
    match e820_type {
        1 => MemoryRegionType::Usable,
        2 => MemoryRegionType::Reserved,
        3 => MemoryRegionType::AcpiReclaimable,
        4 => MemoryRegionType::AcpiNvs,
        5 => MemoryRegionType::Bad,
        _ => MemoryRegionType::Reserved,
    }
}

/// Convertit un type UEFI en MemoryRegionType.
#[inline]
fn uefi_type_to_region_type(uefi_type: u32) -> MemoryRegionType {
    // Types UEFI Memory (EFI_MEMORY_TYPE)
    match uefi_type {
        7  => MemoryRegionType::Usable,           // EfiConventionalMemory
        9  => MemoryRegionType::AcpiReclaimable,  // EfiACPIReclaimMemory
        10 => MemoryRegionType::AcpiNvs,          // EfiACPIMemoryNVS
        11 => MemoryRegionType::Bad,              // EfiMemoryMappedIO → reserved
        _  => MemoryRegionType::Reserved,
    }
}

/// Arrondit `n` vers le haut à l'alignement `align` (doit être une puissance de 2).
#[inline]
const fn align_up(n: u64, align: u64) -> u64 {
    (n + align - 1) & !(align - 1)
}

/// Arrondit `n` vers le bas à l'alignement `align` (doit être une puissance de 2).
#[inline]
const fn align_down(n: u64, align: u64) -> u64 {
    n & !(align - 1)
}

// ─────────────────────────────────────────────────────────────────────────────
// INIT DEPUIS EXO-BOOT (UEFI natif)
// ─────────────────────────────────────────────────────────────────────────────

/// Magic 64-bit du BootInfo exo-boot ("EXOOS_BO" en LE).
pub const EXOBOOT_BOOT_INFO_MAGIC: u64 = 0x4F42_5F53_4F4F_5845;

/// Magic 32-bit transmis en EAX → RDI par exo-boot avant `kernel_main`.
/// Synchronisé avec exo-boot/src/kernel_loader/handoff.rs::EXOBOOT_MAGIC_U32.
pub const EXOBOOT_MAGIC_U32: u32 = 0x4F4F_5845;

// ── Shims repr(C) pour lire le BootInfo d'exo-boot ───────────────────────────

/// Région mémoire exo-boot — layout repr(C), 24 bytes.
/// Miroir exact de `exo-boot::memory::map::MemoryRegion`.
#[repr(C)]
#[derive(Clone, Copy)]
struct ExoMemRegion {
    base:   u64,  // octet  0
    length: u64,  // octet  8
    kind:   u32,  // octet 16 (MemoryKind repr u32)
    _pad:   u32,  // octet 20 (padding → taille totale 24)
}

/// En-tête minimal du BootInfo exo-boot.
/// Miroir partiel de `exo-boot::kernel_loader::handoff::BootInfo` repr(C, align(4096)).
/// Seuls les champs utilisés sont modélisés ; les champs suivants sont ignorés.
#[repr(C, align(4096))]
struct ExoBootInfo {
    magic:               u64,             // offset   0
    version:             u32,             // offset   8
    memory_region_count: u32,             // offset  12
    memory_regions:      [ExoMemRegion; 256], // offset 16, 256 × 24 = 6144 bytes
    // FramebufferInfo (40 bytes, offset 6160)
    fb_phys_addr:        u64,             // offset 6160
    fb_width:            u32,             // offset 6168
    fb_height:           u32,             // offset 6172
    fb_stride:           u32,             // offset 6176
    fb_bpp:              u32,             // offset 6180
    fb_format:           u32,             // offset 6184 (PixelFormat repr u32)
    _fb_pad:             u32,             // offset 6188 (align u64)
    fb_size_bytes:       u64,             // offset 6192
    acpi_rsdp:           u64,             // offset 6200
}

/// Convertit `MemoryKind` exo-boot (repr u32) en `MemoryRegionType` kernel.
#[inline]
fn exo_kind_to_region_type(kind: u32) -> MemoryRegionType {
    match kind {
        1       => MemoryRegionType::Usable,          // Usable
        2 | 3   => MemoryRegionType::KernelImage,     // KernelCode / KernelData
        4       => MemoryRegionType::Usable,          // BootloaderReclaimable → au kernel
        5       => MemoryRegionType::Reserved,        // PageTables (jusqu'à re-init kernel)
        6       => MemoryRegionType::AcpiReclaimable, // AcpiReclaimable
        7       => MemoryRegionType::AcpiNvs,         // AcpiNvs
        8       => MemoryRegionType::Reserved,        // Reserved
        9       => MemoryRegionType::Reserved,        // Framebuffer
        10      => MemoryRegionType::Reserved,        // Mmio
        _       => MemoryRegionType::Reserved,        // Unknown / autres
    }
}

/// Initialise le sous-système mémoire depuis le BootInfo fourni par exo-boot.
///
/// `boot_info_phys` est la valeur transmise par exo-boot dans `mb2_info` (RSI de
/// `kernel_main`), soit l'adresse physique identité-mappée du BootInfo.
///
/// # Safety
/// - `boot_info_phys` doit pointer sur un BootInfo exo-boot valide et aligné sur 4096.
/// - EmergencyPool doit avoir été initialisé avant (RÈGLE MEM-02).
/// - Appel unique depuis le BSP, avant SMP.
pub unsafe fn init_memory_subsystem_exoboot(boot_info_phys: u64) {
    debug_assert!(boot_info_phys != 0, "exo-boot: adresse BootInfo nulle");

    // SAFETY: boot_info_phys est identité-mappé (0–4 GiB, couvert par les tables exo-boot).
    let bi = &*(boot_info_phys as *const ExoBootInfo);

    // Valider le magic avant tout accès supplémentaire
    if bi.magic != EXOBOOT_BOOT_INFO_MAGIC {
        crate::arch::x86_64::halt_cpu();
    }

    let count         = (bi.memory_region_count as usize).min(MAX_MEMORY_REGIONS);
    let mut phys_start = u64::MAX;
    let mut phys_end   = 0u64;
    let mut region_count = 0usize;

    // ── Première passe : bornes physiques + remplissage MEMORY_MAP ────────────
    for i in 0..count {
        let r = &bi.memory_regions[i];
        if r.length == 0 { continue; }

        let base = r.base;
        let end  = base + r.length;
        if end <= PHYS_MEMORY_START { continue; }

        let base_adj = base.max(PHYS_MEMORY_START);

        if base_adj < phys_start { phys_start = base_adj; }
        if end       > phys_end  { phys_end   = end; }

        if region_count < MAX_MEMORY_REGIONS {
            MEMORY_MAP[region_count] = MemoryRegion {
                base:        base_adj,
                size:        end.saturating_sub(base_adj),
                region_type: exo_kind_to_region_type(r.kind),
            };
            region_count += 1;
        }
    }

    MEMORY_REGION_COUNT = region_count;

    if phys_start == u64::MAX || phys_end == 0 {
        crate::arch::x86_64::halt_cpu();
    }

    let phys_start_pa = PhysAddr::new(align_up(phys_start, PAGE_SIZE as u64));
    let phys_end_pa   = PhysAddr::new(align_down(phys_end,  PAGE_SIZE as u64));

    // ── Phase 1 : bitmap allocateur ──────────────────────────────────────────
    init_phase1_bitmap(phys_start_pa, phys_end_pa);

    // ── Phase 2 : libérer les régions utilisables ─────────────────────────────
    for i in 0..count {
        let r = &bi.memory_regions[i];
        if r.length == 0 { continue; }

        match exo_kind_to_region_type(r.kind) {
            MemoryRegionType::Usable | MemoryRegionType::AcpiReclaimable => {}
            _ => continue,
        }

        let base = r.base;
        let end  = base + r.length;
        if end <= PHYS_MEMORY_START { continue; }

        let base_adj = align_up(base.max(PHYS_MEMORY_START), PAGE_SIZE as u64);
        let end_adj  = align_down(end.min(PHYS_MEMORY_MAX),  PAGE_SIZE as u64);
        if base_adj >= end_adj { continue; }

        init_phase2_free_region(PhysAddr::new(base_adj), PhysAddr::new(end_adj));
    }

    // ── Phase 3 : Slab / SLUB ─────────────────────────────────────────────────
    init_phase3_slab_slub();

    // ── Phase 4 : NUMA (nœud 0 par défaut, topologie affinée après ACPI) ─────
    init_phase4_numa(0b0000_0001);
}
