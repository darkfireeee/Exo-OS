//! regions.rs — Détection ACPI RSDP + marquage des régions spéciales.
//!
//! Ce module localise :
//!   - L'ACPI RSDP (Root System Description Pointer) depuis les deux chemins :
//!       * UEFI : EFI Configuration Table (GUID EFI_ACPI_20_TABLE_GUID)
//!       * BIOS : Scan de la zone 0xE0000–0xFFFFF (Extended BIOS Data Area + ROM)
//!   - Le framebuffer GOP (récupéré via `crate::display::framebuffer`)
//!   - Les régions kernel (code/data) après chargement ELF
//!
//! RÈGLE BOOT-04 : Toutes les régions spéciales doivent être explicitement
//!   marquées AVANT de transmettre la carte au kernel.
//!
//! La signature RSDP est "RSD PTR " (8 bytes, incluant l'espace final).
//! Deux versions :
//!   - ACPI 1.0 (20 bytes) — checksum seul
//!   - ACPI 2.0+ (36 bytes) — checksum + extended checksum
//!
//! DOC réf : ACPI Spec 6.5 § 5.2.5 "Root System Description Pointer (RSDP)"

use super::map::{MemoryKind, MemoryRegion, MemoryMap};

// ─── Signature RSDP ──────────────────────────────────────────────────────────

/// Signature ASCII de l'ACPI RSDP : "RSD PTR " (avec espace final).
const RSDP_SIGNATURE: [u8; 8] = *b"RSD PTR ";

/// Structure RSDP ACPI 1.0 (20 bytes).
#[repr(C, packed)]
#[allow(dead_code)]
struct Rsdp1 {
    signature:  [u8; 8],
    checksum:   u8,
    oem_id:     [u8; 6],
    revision:   u8,
    rsdt_addr:  u32,
}

/// Structure RSDP ACPI 2.0+ (36 bytes).
#[repr(C, packed)]
#[allow(dead_code)]
struct Rsdp2 {
    v1:              Rsdp1,
    length:          u32,
    xsdt_addr:       u64,
    ext_checksum:    u8,
    _reserved:       [u8; 3],
}

// ─── Validation RSDP ─────────────────────────────────────────────────────────

/// Valide la checksum RSDP ACPI 1.0 (20 premiers bytes doivent sommer à 0 mod 256).
fn validate_rsdp_v1_checksum(base: *const u8) -> bool {
    // SAFETY : L'appelant garantit que `base` pointe vers ≥ 20 bytes lisibles.
    let sum: u8 = (0..20usize).fold(0u8, |acc, i| acc.wrapping_add(unsafe { *base.add(i) }));
    sum == 0
}

/// Valide la checksum étendue RSDP ACPI 2.0+ (36 premiers bytes).
fn validate_rsdp_v2_checksum(base: *const u8) -> bool {
    // SAFETY : L'appelant garantit que `base` pointe vers ≥ 36 bytes lisibles.
    let sum: u8 = (0..36usize).fold(0u8, |acc, i| acc.wrapping_add(unsafe { *base.add(i) }));
    sum == 0
}

/// Vérifie si l'adresse pointe vers un RSDP valide.
/// Retourne `Some(phys_addr)` si valide, `None` sinon.
///
/// SAFETY : `addr` doit pointer vers de la mémoire lisible (≥ 36 bytes).
unsafe fn probe_rsdp(addr: u64) -> Option<u64> {
    let ptr = addr as *const u8;

    // 1. Vérifie la signature
    let sig = unsafe { core::slice::from_raw_parts(ptr, 8) };
    if sig != RSDP_SIGNATURE {
        return None;
    }

    // 2. Vérifie que revision est connue (0 = ACPI 1.0, ≥ 2 = ACPI 2.0+)
    let revision = unsafe { *ptr.add(15) };

    // 3. Valide la checksum selon la version
    if revision == 0 {
        // ACPI 1.0
        if validate_rsdp_v1_checksum(ptr) {
            return Some(addr);
        }
    } else {
        // ACPI 2.0+ — valide les deux checksums
        if validate_rsdp_v1_checksum(ptr) && validate_rsdp_v2_checksum(ptr) {
            return Some(addr);
        }
    }

    None
}

// ─── Recherche UEFI ───────────────────────────────────────────────────────────

/// GUID EFI_ACPI_20_TABLE_GUID = {8868E871-E4F1-11D3-BC22-0080C73C8881}
#[cfg(feature = "uefi-boot")]
const EFI_ACPI_20_TABLE_GUID: uefi::Guid = uefi::guid!("8868E871-E4F1-11D3-BC22-0080C73C8881");

/// GUID EFI_ACPI_TABLE_GUID (ACPI 1.0) = {EB9D2D30-2D88-11D3-9A16-0090273FC14D}
#[cfg(feature = "uefi-boot")]
const EFI_ACPI_10_TABLE_GUID: uefi::Guid = uefi::guid!("EB9D2D30-2D88-11D3-9A16-0090273FC14D");

/// Recherche le RSDP depuis la Configuration Table UEFI.
///
/// Priorité : ACPI 2.0+ > ACPI 1.0.
/// Cette fonction doit être appelée AVANT ExitBootServices (SystemTable accessible).
#[cfg(feature = "uefi-boot")]
pub fn find_acpi_rsdp_uefi(
    system_table: &uefi::table::SystemTable<uefi::table::Boot>,
) -> Option<u64> {
    let config_tables = system_table.config_table();

    // Priorité 1 : ACPI 2.0+ GUID
    for entry in config_tables.iter() {
        if entry.guid == EFI_ACPI_20_TABLE_GUID {
            let addr = entry.address as u64;
            // SAFETY : L'entrée UEFI pointe vers la table RSDP en mémoire firmware.
            return unsafe { probe_rsdp(addr) };
        }
    }

    // Priorité 2 : ACPI 1.0 GUID
    for entry in config_tables.iter() {
        if entry.guid == EFI_ACPI_10_TABLE_GUID {
            let addr = entry.address as u64;
            // SAFETY : idem.
            return unsafe { probe_rsdp(addr) };
        }
    }

    None
}

// ─── Recherche BIOS ───────────────────────────────────────────────────────────

/// Zone de scan BIOS pour le RSDP (ACPI Spec § 5.2.5.1).
/// L'ACPI spec mentionne deux zones :
///   1. Les premiers 1 KiB de l'Extended BIOS Data Area (EBDA, pointée par 0x40E)
///   2. La zone ROM 0xE0000 – 0xFFFFF (128 KiB) sur limite de 16 bytes
const BIOS_RSDP_SCAN_EBDA_PTR: u64 = 0x40E;  // Pointeur EBDA dans BDA
const BIOS_RSDP_SCAN_ROM_START: u64 = 0xE_0000;
const BIOS_RSDP_SCAN_ROM_END:   u64 = 0x10_0000;
const RSDP_ALIGNMENT: u64 = 16; // RSDP aligné sur 16 bytes

/// Recherche le RSDP dans la zone BIOS.
/// Doit être appelé depuis exoboot_main_bios() après passage en long mode.
pub fn find_acpi_rsdp_bios() -> Option<u64> {
    // ── 1. Tente l'EBDA ───────────────────────────────────────────────────
    let ebda_seg = unsafe {
        // Le mot à 0x40E contient le segment EBDA (multiplié par 16 = adresse physique)
        core::ptr::read_volatile(BIOS_RSDP_SCAN_EBDA_PTR as *const u16) as u64
    };
    let ebda_phys = ebda_seg << 4; // Segment → adresse physique

    // Scan les premiers 1 KiB de l'EBDA sur limites de 16 bytes
    if ebda_phys >= 0x1000 && ebda_phys < 0xA_0000 {
        let ebda_end = ebda_phys + 1024;
        let mut addr = ebda_phys;
        while addr + 20 <= ebda_end {
            // SAFETY : Zone EBDA — mémoire réelle accessible en long mode.
            if let Some(rsdp) = unsafe { probe_rsdp(addr) } {
                return Some(rsdp);
            }
            addr += RSDP_ALIGNMENT;
        }
    }

    // ── 2. Scan la zone ROM 0xE0000 – 0xFFFFF ────────────────────────────
    let mut addr = BIOS_RSDP_SCAN_ROM_START;
    while addr + 20 <= BIOS_RSDP_SCAN_ROM_END {
        // SAFETY : Zone ROM — lisible en long mode (identité-mappée par stage2.asm).
        if let Some(rsdp) = unsafe { probe_rsdp(addr) } {
            return Some(rsdp);
        }
        addr += RSDP_ALIGNMENT;
    }

    None
}

// ─── Marquage des régions spéciales dans MemoryMap ───────────────────────────

/// Options pour le marquage des régions spéciales.
pub struct SpecialRegionsConfig {
    /// Adresse physique du début du code kernel.
    pub kernel_phys_base:  u64,
    /// Taille totale du code + données kernel en bytes.
    pub kernel_phys_size:  u64,
    /// Adresse physique du framebuffer.
    pub framebuffer_base:  Option<u64>,
    /// Taille du framebuffer en bytes.
    pub framebuffer_size:  Option<u64>,
    /// Adresse physique du pool de tables de pages.
    pub page_tables_base:  u64,
    /// Taille du pool de tables de pages.
    pub page_tables_size:  usize,
}

/// Marque les régions spéciales dans la carte mémoire.
///
/// Les régions existantes chevauchantes sont fractionnées si nécessaire.
/// Cette opération produit la carte finale transmise dans BootInfo.
///
/// RÈGLE BOOT-04 : Appelé après collect_*_memory_map() et avant handoff.
pub fn mark_special_regions(
    map: &mut MemoryMap,
    cfg: &SpecialRegionsConfig,
) {
    // Marque le code/données kernel
    if cfg.kernel_phys_size > 0 {
        let _ = map.add_region(MemoryRegion::new(
            cfg.kernel_phys_base,
            cfg.kernel_phys_size,
            MemoryKind::KernelCode,
        ));
    }

    // Marque le framebuffer
    if let (Some(fb_base), Some(fb_size)) = (cfg.framebuffer_base, cfg.framebuffer_size) {
        if fb_size > 0 {
            let _ = map.add_region(MemoryRegion::new(
                fb_base,
                fb_size,
                MemoryKind::Framebuffer,
            ));
        }
    }

    // Marque le pool de tables de pages
    if cfg.page_tables_size > 0 {
        let _ = map.add_region(MemoryRegion::new(
            cfg.page_tables_base,
            cfg.page_tables_size as u64,
            MemoryKind::PageTables,
        ));
    }
}

/// Marque la région contenant BootInfo lui-même dans la carte.
/// BootInfo est alloué par le bootloader — kernel peut le reclamer après lecture.
pub fn mark_boot_info_region(map: &mut MemoryMap, boot_info_phys: u64, boot_info_size: usize) {
    let _ = map.add_region(MemoryRegion::new(
        boot_info_phys,
        boot_info_size as u64,
        MemoryKind::BootloaderReclaimable,
    ));
}
