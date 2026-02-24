//! # arch/x86_64/boot/multiboot2.rs — Parseur Multiboot2
//!
//! Parse les tags Multiboot2 pour extraire :
//! - La memory map (mémoire disponible, réservée, ACPI, etc.)
//! - L'adresse RSDP
//! - Le nom du bootloader
//! - La command line
//!
//! ## Format Multiboot2
//! Structure à l'adresse passée par le bootloader :
//! - `total_size` (u32) + `reserved` (u32) = 8 octets header
//! - Tags de longueur variable, alignés sur 8 octets
//! - Tag type 0 = fin de liste

#![allow(dead_code)]

// ── Types de tags Multiboot2 ──────────────────────────────────────────────────

const TAG_END:         u32 = 0;
const TAG_CMDLINE:     u32 = 1;
const TAG_BOOTLOADER:  u32 = 2;
const TAG_MODULE:      u32 = 3;
const TAG_BASIC_MEMINFO:u32 = 4;
const TAG_MMAP:        u32 = 6;
const TAG_FRAMEBUFFER: u32 = 8;
const TAG_ELF_SECTIONS:u32 = 9;
const TAG_APM_TABLE:   u32 = 10;
const TAG_RSDP_V1:     u32 = 14;  // ACPI 1.0 RSDP
const TAG_RSDP_V2:     u32 = 15;  // ACPI 2.0+ RSDP
const TAG_EFI_MMAP:    u32 = 17;
const TAG_EFI64_IMAGE: u32 = 18;

// ── Memory map entry types ────────────────────────────────────────────────────

pub const MMAP_AVAILABLE: u32 = 1;
pub const MMAP_RESERVED:  u32 = 2;
pub const MMAP_ACPI:      u32 = 3;
pub const MMAP_HIBERNATE: u32 = 4;
pub const MMAP_DEFECTIVE: u32 = 5;

// ── Structures ────────────────────────────────────────────────────────────────

#[repr(C, packed)]
struct Mb2Header {
    total_size: u32,
    reserved:   u32,
}

#[repr(C, packed)]
struct Mb2Tag {
    tag_type: u32,
    size:     u32,
}

/// Entrée de la memory map Multiboot2
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct MmapEntry {
    pub base_addr: u64,
    pub length:    u64,
    pub entry_type:u32,
    pub _reserved: u32,
}

/// Informations extraites de la structure Multiboot2
#[derive(Debug, Clone, Copy)]
pub struct Multiboot2Info {
    pub rsdp_phys:      u64,
    pub total_memory_kb:u64,
    pub cmdline_ptr:    u64,  // pointeur vers la string null-terminée
    pub bootloader_ptr: u64,
    pub mmap_ptr:       u64,  // pointeur vers le first MmapEntry
    pub mmap_count:     u32,
}

impl Multiboot2Info {
    const fn zeroed() -> Self {
        Self { rsdp_phys: 0, total_memory_kb: 0, cmdline_ptr: 0,
               bootloader_ptr: 0, mmap_ptr: 0, mmap_count: 0 }
    }
}

// ── Parseur ───────────────────────────────────────────────────────────────────

/// Parse la structure Multiboot2 à l'adresse physique `mb2_phys`
///
/// # Safety
/// L'adresse doit être valide et en mémoire identity-mappée.
pub fn parse_multiboot2(mb2_phys: u64) -> Multiboot2Info {
    let mut info = Multiboot2Info::zeroed();
    if mb2_phys == 0 { return info; }

    // SAFETY: adresse passée par le BSP bootloader, identity-mappée
    let header = unsafe { &*(mb2_phys as *const Mb2Header) };
    let total_size = header.total_size as usize;
    if total_size < 8 { return info; }

    let mut offset: usize = 8; // après le header Mb2Header

    while offset + 8 <= total_size {
        let tag_addr = mb2_phys as usize + offset;
        // SAFETY: offset dans la structure Multiboot2 (total_size vérifié)
        let tag = unsafe { &*(tag_addr as *const Mb2Tag) };
        if tag.tag_type == TAG_END { break; }

        let tag_size = tag.size as usize;
        if tag_size < 8 { break; }

        let data_addr = tag_addr + 8; // après les 8 octets type+size

        match tag.tag_type {
            TAG_CMDLINE => {
                info.cmdline_ptr = data_addr as u64;
            }
            TAG_BOOTLOADER => {
                info.bootloader_ptr = data_addr as u64;
            }
            TAG_BASIC_MEMINFO => {
                // u32 mem_lower (KiB en dessous de 1MiB) + u32 mem_upper (KiB au-dessus)
                // SAFETY: taille fixe connue
                let lower = unsafe { core::ptr::read_volatile(data_addr as *const u32) } as u64;
                let upper = unsafe { core::ptr::read_volatile((data_addr + 4) as *const u32) } as u64;
                info.total_memory_kb = lower + upper;
            }
            TAG_MMAP => {
                // u32 entry_size + u32 entry_version + entries...
                let entry_size = unsafe { core::ptr::read_volatile(data_addr as *const u32) } as usize;
                if entry_size < core::mem::size_of::<MmapEntry>() { break; }
                let entries_start = data_addr + 8;
                let entries_end   = tag_addr + tag_size;
                let n_entries = (entries_end - entries_start) / entry_size;
                info.mmap_ptr   = entries_start as u64;
                info.mmap_count = n_entries as u32;

                // Calculer total mémoire disponible
                for i in 0..n_entries {
                    let entry_addr = entries_start + i * entry_size;
                    // SAFETY: index dans la table mémoire
                    let entry = unsafe { &*(entry_addr as *const MmapEntry) };
                    if entry.entry_type == MMAP_AVAILABLE {
                        info.total_memory_kb += entry.length / 1024;
                    }
                }
            }
            TAG_RSDP_V1 | TAG_RSDP_V2 => {
                info.rsdp_phys = data_addr as u64;
            }
            _ => {}
        }

        // Aligner l'offset suivant sur 8 octets
        offset += (tag_size + 7) & !7;
    }

    info
}

/// Retourne un slice des entrées memory map depuis un `Multiboot2Info`
pub fn mmap_entries(info: &Multiboot2Info) -> &'static [MmapEntry] {
    if info.mmap_ptr == 0 || info.mmap_count == 0 { return &[]; }
    // SAFETY: adresses validées lors du parsing
    unsafe {
        core::slice::from_raw_parts(info.mmap_ptr as *const MmapEntry, info.mmap_count as usize)
    }
}
