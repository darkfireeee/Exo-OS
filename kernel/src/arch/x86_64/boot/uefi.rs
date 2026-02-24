//! # arch/x86_64/boot/uefi.rs — Support protocole UEFI (memory map)
//!
//! Exo-OS peut être démarré via UEFI (directement ou via un bootloader UEFI
//! comme GRUB2 EFI ou Limine). Dans ce cas, la memory map est fournie par
//! ExitBootServices() sous forme d'une table de descripteurs EFI.

#![allow(dead_code)]

// ── Types mémoire EFI ─────────────────────────────────────────────────────────

pub const EFI_CONVENTIONAL_MEMORY:  u32 = 7;
pub const EFI_LOADER_CODE:          u32 = 1;
pub const EFI_LOADER_DATA:          u32 = 2;
pub const EFI_BOOT_SERVICES_CODE:   u32 = 3;
pub const EFI_BOOT_SERVICES_DATA:   u32 = 4;
pub const EFI_RUNTIME_SERVICES_CODE:u32 = 5;
pub const EFI_RUNTIME_SERVICES_DATA:u32 = 6;
pub const EFI_UNUSABLE_MEMORY:      u32 = 8;
pub const EFI_ACPI_RECLAIM:         u32 = 9;
pub const EFI_ACPI_MEMORY_NVS:      u32 = 10;
pub const EFI_MEMORY_MAPPED_IO:     u32 = 11;

/// Attributs EFI Memory
pub const EFI_MEMORY_WB: u64 = 1 << 3; // Write-Back
pub const EFI_MEMORY_WC: u64 = 1 << 1; // Write-Combine
pub const EFI_MEMORY_UC: u64 = 1 << 0; // Uncacheable

// ── Descripteur EFI Memory ────────────────────────────────────────────────────

/// EFI_MEMORY_DESCRIPTOR (taille variable selon firmware, desc_size indique la taille réelle)
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct EfiMemoryDescriptor {
    pub mem_type:        u32,
    pub _pad:            u32,
    pub physical_start:  u64,
    pub virtual_start:   u64,
    pub number_of_pages: u64,
    pub attribute:       u64,
}

impl EfiMemoryDescriptor {
    /// `true` si la région est de la RAM conventionnelle utilisable
    pub fn is_usable(&self) -> bool {
        matches!(
            self.mem_type,
            EFI_CONVENTIONAL_MEMORY |
            EFI_LOADER_CODE | EFI_LOADER_DATA |
            EFI_BOOT_SERVICES_CODE | EFI_BOOT_SERVICES_DATA
        )
    }

    /// Taille de la région en octets
    pub fn size_bytes(&self) -> u64 {
        self.number_of_pages * 4096
    }
}

// ── Memory map UEFI ───────────────────────────────────────────────────────────

/// Memory map UEFI — référence à la table fournie par le bootloader
#[derive(Debug, Clone, Copy)]
pub struct UefiMemoryMap {
    pub base_addr:   u64,  // Adresse physique du premier descripteur
    pub map_size:    u64,  // Taille totale en octets
    pub desc_size:   u32,  // Taille d'un descripteur (peut différer de sizeof EfiMemoryDescriptor)
    pub desc_version:u32,  // Version (1)
    pub total_memory_kb: u64,
    pub entry_count: u32,
}

impl UefiMemoryMap {
    pub const fn zeroed() -> Self {
        Self { base_addr: 0, map_size: 0, desc_size: 0, desc_version: 0,
               total_memory_kb: 0, entry_count: 0 }
    }

    /// Itère les descripteurs EFI
    pub fn iter(&self) -> UefiMemMapIterator {
        UefiMemMapIterator { map: *self, offset: 0 }
    }
}

pub struct UefiMemMapIterator {
    map:    UefiMemoryMap,
    offset: u64,
}

impl Iterator for UefiMemMapIterator {
    type Item = EfiMemoryDescriptor;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.map.map_size { return None; }
        let addr = self.map.base_addr + self.offset;
        // SAFETY: adresse dans la memory map UEFI, taille desc_size validée
        let desc = unsafe { core::ptr::read_volatile(addr as *const EfiMemoryDescriptor) };
        self.offset += self.map.desc_size as u64;
        Some(desc)
    }
}

// ── Parseur ───────────────────────────────────────────────────────────────────

/// Construit un `UefiMemoryMap` depuis les paramètres de ExitBootServices
///
/// Appelé depuis le bootloader stub ou depuis le tag Multiboot2 EFI.
pub fn parse_uefi_memmap(
    map_addr:    u64,
    map_size:    u64,
    desc_size:   u32,
    desc_version:u32,
) -> UefiMemoryMap {
    let mut uefi = UefiMemoryMap {
        base_addr: map_addr,
        map_size,
        desc_size,
        desc_version,
        total_memory_kb: 0,
        entry_count: if desc_size > 0 { (map_size / desc_size as u64) as u32 } else { 0 },
    };

    // Calculer la mémoire disponible totale
    let mut total_kb: u64 = 0;
    for desc in uefi.iter() {
        if desc.is_usable() {
            total_kb += desc.size_bytes() / 1024;
        }
    }
    uefi.total_memory_kb = total_kb;
    uefi
}
