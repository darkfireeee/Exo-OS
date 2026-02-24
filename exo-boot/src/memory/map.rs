//! map.rs — Consolidation E820 + UEFI Memory Map → format unifié BootInfo.
//!
//! RÈGLE BOOT-04 (DOC10) :
//!   "Mémoire identifiée au type EXACT avant handoff.
//!    KernelCode ≠ KernelData ≠ Bootloader reclaimable."
//!
//! Ce module convertit deux sources de carte mémoire en un format unique :
//!   - UEFI Memory Map (GetMemoryMap()) → utilisé sur x86_64-unknown-uefi
//!   - E820 (INT 15h AX=E820h par stage2) → utilisé sur BIOS
//!
//! Format final (MemoryRegion[]) transmis dans BootInfo.
//! Le kernel (kernel/src/arch/x86_64/boot/early_init.rs) range ce tableau
//! dans son buddy allocator.
//!
//! CONTRAT handoff : Ce format est le contrat formel bootloader→kernel.
//! Tout changement ici DOIT être synchronisé avec early_init.rs.

use super::{MAX_MEMORY_REGIONS, PAGE_SIZE};
use arrayvec::ArrayVec;

// ─── Types de régions mémoire ─────────────────────────────────────────────────

/// Classification des régions de la carte mémoire.
///
/// RÈGLE BOOT-04 : chaque type doit être distinct et non ambigu.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    /// RAM libre — donnée au buddy allocator du kernel.
    Usable          = 1,
    /// Code et données du kernel chargés par le bootloader.
    KernelCode      = 2,
    /// Stack et tas initiaux du kernel.
    KernelData      = 3,
    /// Code du bootloader — peut être réclamé par le kernel après init.
    /// Le kernel doit libérer ces pages après avoir traité BootInfo.
    BootloaderReclaimable = 4,
    /// Tables de pages initiales (bootloader) — reclaimable après kernel init.
    PageTables      = 5,
    /// Données ACPI — le kernel peut les lire puis les libérer.
    AcpiReclaimable = 6,
    /// Données ACPI non-volatiles (NVRAM) — préservées.
    AcpiNvs         = 7,
    /// Région défectueuse ou réservée — NE PAS allouer.
    Reserved        = 8,
    /// Framebuffer GOP — mappé par kernel/fs/tty.
    Framebuffer     = 9,
    /// Espace MMIO matériel — NE PAS allouer.
    Mmio            = 10,
    /// Région inconnue ou firmware-spécifique — NE PAS allouer par précaution.
    Unknown         = 255,
}

impl MemoryKind {
    /// Retourne `true` si cette région peut être allouée par le kernel.
    #[inline]
    pub fn is_usable_by_kernel(self) -> bool {
        matches!(self, Self::Usable | Self::BootloaderReclaimable | Self::AcpiReclaimable)
    }

    /// Retourne `true` si cette région doit être préservée (jamais écrasée).
    #[inline]
    pub fn must_preserve(self) -> bool {
        matches!(self, Self::AcpiNvs | Self::Reserved | Self::Mmio | Self::Framebuffer)
    }
}

// ─── Structure de région mémoire ─────────────────────────────────────────────

/// Région mémoire dans la carte du bootloader.
///
/// RÈGLE BOOT-03 : tous les champs initialisés (zéro-fill si non utilisé).
/// `repr(C)` : layout ABI stable pour le passage au kernel.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MemoryRegion {
    /// Adresse physique de début (alignée page — RÈGLE : toujours multiple de PAGE_SIZE).
    pub base:   u64,
    /// Taille en octets (multiple de PAGE_SIZE).
    pub length: u64,
    /// Type de région.
    pub kind:   MemoryKind,
    /// Padding pour aligner à 16 bytes (ABI stable).
    pub _pad:   u32,
}

const _: () = assert!(core::mem::size_of::<MemoryRegion>() == 24, "MemoryRegion doit faire 24 bytes");

impl MemoryRegion {
    /// Construit une `MemoryRegion` avec page-alignment.
    pub fn new(base: u64, length: u64, kind: MemoryKind) -> Self {
        // Aligne la base au-dessus et la fin en-dessous
        let aligned_base   = align_up(base, PAGE_SIZE as u64);
        let end            = base + length;
        let aligned_end    = align_down(end, PAGE_SIZE as u64);
        let aligned_length = aligned_end.saturating_sub(aligned_base);

        Self {
            base:   aligned_base,
            length: aligned_length,
            kind,
            _pad:   0,
        }
    }

    /// Adresse physique de fin de la région.
    #[inline]
    pub fn end(&self) -> u64 { self.base + self.length }

    /// `true` si cette région contient l'adresse physique `addr`.
    #[inline]
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.base && addr < self.end()
    }

    /// `true` si la région est valide (base et length alignés, non nuls).
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.length > 0 && self.base % PAGE_SIZE as u64 == 0
    }
}

// ─── MemoryMap ────────────────────────────────────────────────────────────────

/// Carte mémoire consolidée du bootloader.
/// Taille statique — aucune allocation heap nécessaire.
pub struct MemoryMap {
    regions: ArrayVec<MemoryRegion, MAX_MEMORY_REGIONS>,
    /// RAM totale libre disponible pour le kernel (en octets).
    total_usable: u64,
    /// Nombre total d'octets répertoriés (tous types confondus).
    total_detected: u64,
}

impl MemoryMap {
    fn new() -> Self {
        Self {
            regions:        ArrayVec::new(),
            total_usable:   0,
            total_detected: 0,
        }
    }

    /// Ajoute une région en la fusionnant avec les régions adjacentes du même type si possible.
    pub fn add_region(&mut self, region: MemoryRegion) -> Result<(), MemoryMapError> {
        if !region.is_valid() { return Ok(()); } // Ignore les régions invalides
        if region.length == 0 { return Ok(()); }

        // Trie par adresse de base
        let insert_pos = self.regions.partition_point(|r| r.base < region.base);

        if self.regions.is_full() {
            // Essaie de fusionner avec une région adjacente avant d'insérer
            if insert_pos > 0 {
                let prev = &mut self.regions[insert_pos - 1];
                if prev.kind == region.kind && prev.end() == region.base {
                    prev.length += region.length;
                    self.update_totals(&region);
                    return Ok(());
                }
            }
            return Err(MemoryMapError::TooManyRegions {
                count: self.regions.len(),
                max:   MAX_MEMORY_REGIONS,
            });
        }

        self.update_totals(&region);
        // SAFETY : Vérifié non plein ci-dessus.
        self.regions.insert(insert_pos, region);
        Ok(())
    }

    fn update_totals(&mut self, region: &MemoryRegion) {
        self.total_detected += region.length;
        if region.kind.is_usable_by_kernel() {
            self.total_usable += region.length;
        }
    }

    /// Retourne le slice des régions pour BootInfo.
    #[inline]
    pub fn regions_slice(&self) -> &[MemoryRegion] {
        self.regions.as_slice()
    }

    /// RAM libre disponible pour le kernel (bytes).
    #[inline]
    pub fn total_usable_bytes(&self) -> u64 { self.total_usable }

    /// Nombre de régions dans la carte.
    #[inline]
    pub fn region_count(&self) -> usize { self.regions.len() }
}

// ─── Conversion UEFI Memory Map ───────────────────────────────────────────────

/// Carte mémoire UEFI brute avant conversion.
#[cfg(feature = "uefi-boot")]
pub struct RawUefiMemoryMap {
    pub entries: arrayvec::ArrayVec<crate::uefi::services::UefiMemoryDescriptorCompact, 1024>,
    pub key:     usize,
}

/// Convertit la carte mémoire UEFI brute en MemoryMap Exo-OS.
/// Retourne le key UEFI (pour ExitBootServices) et la MemoryMap.
///
/// RÈGLE BOOT-04 : Les types UEFI sont mappés vers les types Exo-OS EXACTEMENT.
#[cfg(feature = "uefi-boot")]
pub fn convert_uefi_memory_map(raw: RawUefiMemoryMap) -> (usize, MemoryMap) {
    let mut map = MemoryMap::new();

    for desc in &raw.entries {
        let kind = uefi_type_to_exo_kind(desc.memory_type);
        let region = MemoryRegion::new(
            desc.physical_start,
            desc.number_of_pages * PAGE_SIZE as u64,
            kind,
        );
        let _ = map.add_region(region);
    }

    (raw.key, map)
}

/// Collecte la UEFI memory map via GetMemoryMap().
#[cfg(feature = "uefi-boot")]
pub fn collect_uefi_memory_map(
    bt: &uefi::table::boot::BootServices,
) -> Result<RawUefiMemoryMap, MemoryMapError> {
    let raw_buf = crate::uefi::services::get_memory_map_raw(bt)
        .map_err(|_| MemoryMapError::CollectFailed)?;
    Ok(RawUefiMemoryMap {
        entries: raw_buf.entries,
        key:     raw_buf.key,
    })
}

/// Mappe un type mémoire UEFI (EFI_MEMORY_TYPE) vers un MemoryKind Exo-OS.
/// Basé sur la spec UEFI 2.10 § 7.2 Memory Allocation Services.
fn uefi_type_to_exo_kind(uefi_type: u32) -> MemoryKind {
    match uefi_type {
        0  => MemoryKind::Reserved,          // EfiReservedMemoryType
        1  => MemoryKind::BootloaderReclaimable, // EfiLoaderCode (bootloader lui-même)
        2  => MemoryKind::BootloaderReclaimable, // EfiLoaderData
        3  => MemoryKind::BootloaderReclaimable, // EfiBootServicesCode
        4  => MemoryKind::BootloaderReclaimable, // EfiBootServicesData
        5  => MemoryKind::Reserved,          // EfiRuntimeServicesCode — préserver
        6  => MemoryKind::Reserved,          // EfiRuntimeServicesData — préserver
        7  => MemoryKind::Usable,            // EfiConventionalMemory — RAM libre
        8  => MemoryKind::Reserved,          // EfiUnusableMemory — défectueuse
        9  => MemoryKind::AcpiReclaimable,   // EfiACPIReclaimMemory
        10 => MemoryKind::AcpiNvs,           // EfiACPIMemoryNVS
        11 => MemoryKind::Mmio,              // EfiMemoryMappedIO
        12 => MemoryKind::Mmio,              // EfiMemoryMappedIOPortSpace
        13 => MemoryKind::Reserved,          // EfiPalCode (Itanium PAL — x86 use)
        14 => MemoryKind::Usable,            // EfiPersistentMemory (NVDIMM libre)
        _  => MemoryKind::Unknown,
    }
}

// ─── Conversion E820 (BIOS) ───────────────────────────────────────────────────

/// Structure d'une entrée E820 telle que remplie par stage2.asm.
/// Doit correspondre EXACTEMENT au format utilisé dans detect_e820 (stage2.asm).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct E820Entry {
    /// Adresse de base de la région.
    pub base:   u64,
    /// Taille de la région en octets.
    pub length: u64,
    /// Type E820 (1=Usable, 2=Reserved, 3=ACPI Reclaimable, 4=ACPI NVS, 5=Bad).
    pub kind:   u32,
    /// Extension ACPI 3.0 (bit 0 : ignore si 0, bit 1 : non volatile).
    pub acpi3_attrs: u32,
}

/// Construit une MemoryMap depuis le buffer E820 rempli par stage2.asm.
///
/// SAFETY : `buffer_addr` doit pointer vers un tableau valide de `entry_count`
/// structures `E820Entry`. Garantie par le contrat avec stage2.asm.
pub fn collect_bios_memory_map(
    buffer_addr:  u64,
    entry_count:  u32,
) -> Result<MemoryMap, MemoryMapError> {
    let mut map = MemoryMap::new();

    if entry_count == 0 {
        return Err(MemoryMapError::Empty);
    }
    if entry_count > MAX_MEMORY_REGIONS as u32 {
        return Err(MemoryMapError::TooManyRegions {
            count: entry_count as usize,
            max:   MAX_MEMORY_REGIONS,
        });
    }

    // SAFETY : Garantie par le contrat avec stage2.asm — buffer valide et taille connue.
    let entries = unsafe {
        core::slice::from_raw_parts(
            buffer_addr as *const E820Entry,
            entry_count as usize,
        )
    };

    for entry in entries {
        // Ignore les entrées avec ACPI3 Attribute Bit 0 = 0 (ignorer si absent)
        // Bit 0 de acpi3_attrs = 0 ET l'entrée a des attrs → ignorer
        // (Bit 0 = 1 ou pas d'attrs ACPI3 → traiter)
        if entry.acpi3_attrs & 1 == 0 && entry.acpi3_attrs != 0 {
            continue;
        }

        let kind = e820_type_to_exo_kind(entry.kind);
        let region = MemoryRegion::new(entry.base, entry.length, kind);
        map.add_region(region)?;
    }

    if map.region_count() == 0 {
        return Err(MemoryMapError::Empty);
    }

    Ok(map)
}

/// Mappe un type E820 vers un MemoryKind Exo-OS.
fn e820_type_to_exo_kind(e820_type: u32) -> MemoryKind {
    match e820_type {
        1 => MemoryKind::Usable,
        2 => MemoryKind::Reserved,
        3 => MemoryKind::AcpiReclaimable,
        4 => MemoryKind::AcpiNvs,
        5 => MemoryKind::Reserved,       // E820_TYPE_UNUSABLE (défectueux)
        _ => MemoryKind::Unknown,
    }
}

// ─── Helpers arithmétiques ─────────────────────────────────────────────────────

/// Aligne `val` vers le haut sur `align` (doit être une puissance de 2).
#[inline]
pub fn align_up(val: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (val + align - 1) & !(align - 1)
}

/// Aligne `val` vers le bas sur `align` (doit être une puissance de 2).
#[inline]
pub fn align_down(val: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    val & !(align - 1)
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum MemoryMapError {
    /// Aucune entrée de mémoire (firmware ou buffer invalide).
    Empty,
    /// Plus de MAX_MEMORY_REGIONS régions — configuration mémoire extrêmement fragmentée.
    TooManyRegions { count: usize, max: usize },
    /// Erreur lors de la collecte depuis le firmware (UEFI GetMemoryMap failed).
    CollectFailed,
}

impl core::fmt::Display for MemoryMapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "Carte mémoire vide — firmware défectueux"),
            Self::TooManyRegions { count, max } =>
                write!(f, "Trop de régions mémoire : {} > {}", count, max),
            Self::CollectFailed =>
                write!(f, "Échec de GetMemoryMap() UEFI"),
        }
    }
}
