//! # arch/x86_64/acpi/parser.rs — Parseur RSDP / XSDT / RSDT
//!
//! Localise et parse les tables ACPI racines pour découvrir
//! les tables filles (MADT, HPET, FADT, SRAT, etc.).
//!
//! ## Séquence de découverte
//! 1. Localiser le RSDP dans le segment EBDA (0xE0000–0xFFFFF)
//!    ou passé par le bootloader (Multiboot2 / UEFI)
//! 2. Lire la version : ACPI 1.0 → RSDT (32 bits), ACPI 2.0+ → XSDT (64 bits)
//! 3. Itérer les entrées pour localiser chaque table par signature 4-octet

#![allow(dead_code)]

use core::ptr::read_volatile;
use core::sync::atomic::{AtomicU64, Ordering};

// ── Structures ACPI ───────────────────────────────────────────────────────────

/// Root System Description Pointer (RSDP) — ACPI 2.0+
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Rsdp {
    pub signature:  [u8; 8],  // "RSD PTR "
    pub checksum:   u8,
    pub oem_id:     [u8; 6],
    pub revision:   u8,       // 0 = ACPI 1.0, 2 = ACPI 2.0+
    pub rsdt_addr:  u32,      // Adresse physique RSDT (32 bits)
    // Champs ACPI 2.0+ (revision >= 2)
    pub length:     u32,
    pub xsdt_addr:  u64,      // Adresse physique XSDT (64 bits)
    pub ext_checksum: u8,
    pub _reserved:  [u8; 3],
}

/// System Description Table Header (commun à toutes les tables)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct SdtHeader {
    pub signature:       [u8; 4],
    pub length:          u32,
    pub revision:        u8,
    pub checksum:        u8,
    pub oem_id:          [u8; 6],
    pub oem_table_id:    [u8; 8],
    pub oem_revision:    u32,
    pub creator_id:      u32,
    pub creator_revision:u32,
}

impl SdtHeader {
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }

    /// Valide le checksum : somme de tous les octets = 0 mod 256
    pub fn valid_checksum(&self) -> bool {
        let len = self.length as usize;
        // SAFETY: structure en mémoire ACPI, longueur validée par le parseur ACPI avant tout appel.
        let bytes = unsafe {
            core::slice::from_raw_parts(self as *const _ as *const u8, len)
        };
        bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b)) == 0
    }
}

// ── Informations globales ACPI ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct AcpiInfo {
    pub acpi_version:  u8,
    pub rsdp_phys:     u64,
    pub xsdt_phys:     u64,
    pub rsdt_phys:     u32,
    pub madt_phys:     u64,
    pub hpet_phys:     u64,
    pub fadt_phys:     u64,
    pub srat_phys:     u64,
}

impl AcpiInfo {
    const fn zeroed() -> Self {
        Self { acpi_version: 0, rsdp_phys: 0, xsdt_phys: 0, rsdt_phys: 0,
               madt_phys: 0, hpet_phys: 0, fadt_phys: 0, srat_phys: 0 }
    }
}

static ACPI_INFO_ADDR: AtomicU64 = AtomicU64::new(0);
// Stockage statique de AcpiInfo (initialisé une seule fois au boot)
struct AcpiInfoCell(core::cell::UnsafeCell<AcpiInfo>);
unsafe impl Sync for AcpiInfoCell {}

static ACPI_INFO: AcpiInfoCell =
    AcpiInfoCell(core::cell::UnsafeCell::new(AcpiInfo::zeroed()));

/// Retourne l'AcpiInfo globale (valide après `init_acpi()`)
pub fn acpi_info() -> &'static AcpiInfo {
    // SAFETY: initialisé avant toute utilisation des structures ACPI
    unsafe { &*ACPI_INFO.0.get() }
}

/// Retourne `true` si ACPI a été initialisé avec succès
///
/// Vérifié via ACPI_INFO_ADDR != 0 (mis à jour par `init_acpi_from_rsdp()`).
#[inline(always)]
pub fn acpi_available() -> bool {
    ACPI_INFO_ADDR.load(Ordering::Relaxed) != 0
}

// ── Localisation RSDP ─────────────────────────────────────────────────────────

const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// Recherche le RSDP dans une plage mémoire (scan octet par octet, aligné 16)
fn find_rsdp_in_range(start: u64, end: u64) -> Option<u64> {
    let mut addr = start & !15; // aligné 16
    while addr + 20 <= end {
        let ptr = addr as *const [u8; 8];
        // SAFETY: adresse dans Low Memory (EBDA / ROM BIOS) — mapped identity
        let sig = unsafe { read_volatile(ptr) };
        if &sig == RSDP_SIGNATURE {
            // SAFETY: addr est aligné 16 B et dans Low Memory identity-mapped ;
            // on vient de vérifier la signature, la structure Rsdp commence ici.
            let rsdp = unsafe { &*(addr as *const Rsdp) };
            // Valider le checksum des 20 premiers octets
            // SAFETY: même invariant — 20 B dès `addr` sont lisibles (addr+20 <= end vérifié).
            let bytes20 = unsafe { core::slice::from_raw_parts(addr as *const u8, 20) };
            let sum = bytes20.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            if sum == 0 { return Some(addr); }
        }
        addr += 16;
    }
    None
}

/// Localise le RSDP dans l'EBDA et la ROM zone BIOS
pub fn find_rsdp() -> Option<u64> {
    // 1. Chercher dans l'EBDA (Extended BIOS Data Area)
    // SAFETY: accès Low Memory — identity-mapped par le boot
    let ebda_segment = unsafe { read_volatile(0x40E as *const u16) } as u64;
    let ebda_base = ebda_segment << 4;
    if ebda_base >= 0x80000 && ebda_base < 0xA0000 {
        if let Some(a) = find_rsdp_in_range(ebda_base, ebda_base + 1024) {
            return Some(a);
        }
    }
    // 2. Zone ROM BIOS 0xE0000–0xFFFFF
    find_rsdp_in_range(0xE0000, 0x100000)
}

// ── Initialisation principale ─────────────────────────────────────────────────

/// Initialise ACPI depuis une adresse RSDP connue (passée par le bootloader)
///
/// Appelé depuis `boot::early_init` avec l'adresse RSDP fournie par Multiboot2 / UEFI.
pub fn init_acpi_from_rsdp(rsdp_phys: u64) {
    // SAFETY: ACPI_INFO est une UnsafeCell statique, initialisée une seule fois lors du boot
    // (pas de reentrée possible : appelé un seul fois avant tout SMP).
    let info = unsafe { &mut *ACPI_INFO.0.get() };
    info.rsdp_phys = rsdp_phys;

    // SAFETY: adresse passée par le bootloader — validée par le parseur BIOS/UEFI
    let rsdp = unsafe { &*(rsdp_phys as *const Rsdp) };
    info.acpi_version = rsdp.revision;

    if rsdp.revision >= 2 && rsdp.xsdt_addr != 0 {
        info.xsdt_phys = rsdp.xsdt_addr;
        parse_xsdt(rsdp.xsdt_addr, info);
    } else if rsdp.rsdt_addr != 0 {
        info.rsdt_phys = rsdp.rsdt_addr;
        parse_rsdt(rsdp.rsdt_addr as u64, info);
    }

    ACPI_INFO_ADDR.store(rsdp_phys, Ordering::Release);
}

/// Initialise ACPI en auto-détection (scan EBDA/ROM si pas de pointeur bootloader)
pub fn init_acpi() -> Option<AcpiInfo> {
    let rsdp_addr = find_rsdp()?;
    init_acpi_from_rsdp(rsdp_addr);
    Some(*acpi_info())
}

// ── Parseur XSDT ─────────────────────────────────────────────────────────────

fn parse_xsdt(xsdt_phys: u64, info: &mut AcpiInfo) {
    // SAFETY: adresse XSDT validée par le RSDP
    let header = unsafe { &*(xsdt_phys as *const SdtHeader) };
    if &header.signature != b"XSDT" { return; }

    let n_entries = (header.length as usize - core::mem::size_of::<SdtHeader>()) / 8;
    let entries_base = xsdt_phys as usize + core::mem::size_of::<SdtHeader>();

    for i in 0..n_entries {
        // SAFETY: index dans la table XSDT (longueur validée)
        let entry_addr = unsafe { read_volatile((entries_base + i * 8) as *const u64) };
        classify_table(entry_addr, info);
    }
}

fn parse_rsdt(rsdt_phys: u64, info: &mut AcpiInfo) {
    // SAFETY: adresse RSDT validée par le RSDP
    let header = unsafe { &*(rsdt_phys as *const SdtHeader) };
    if &header.signature != b"RSDT" { return; }

    let n_entries = (header.length as usize - core::mem::size_of::<SdtHeader>()) / 4;
    let entries_base = rsdt_phys as usize + core::mem::size_of::<SdtHeader>();

    for i in 0..n_entries {
        // SAFETY: index dans la table RSDT (longueur validée)
        let entry_addr32 = unsafe { read_volatile((entries_base + i * 4) as *const u32) };
        classify_table(entry_addr32 as u64, info);
    }
}

fn classify_table(phys: u64, info: &mut AcpiInfo) {
    if phys == 0 { return; }
    // SAFETY: adresse dans les tables ACPI — identity-mapped ou mappée par le boot
    let sig = unsafe { read_volatile(phys as *const [u8; 4]) };
    match &sig {
        b"APIC" => info.madt_phys = phys,
        b"HPET" => info.hpet_phys = phys,
        b"FACP" => info.fadt_phys = phys,
        b"SRAT" => info.srat_phys = phys,
        _       => {}
    }
}
