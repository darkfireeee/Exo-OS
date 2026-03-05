//! # arch/x86_64/acpi/madt.rs — Multiple APIC Description Table
//!
//! Parse la MADT pour extraire :
//! - Les LAPIC IDs de chaque CPU logique
//! - Les I/O APIC (base MMIO + GSI base)
//! - Les interruption source overrides (ISA IRQ → GSI)
//! - L'adresse LAPIC override (si différente de 0xFEE00000)
//!
//! ## Format MADT
//! Header SdtHeader + LAPIC_ADDR (32 bits) + FLAGS (32 bits)
//! + liste de structures de longueur variable (type, length, ...)

#![allow(dead_code)]

use core::ptr::read_volatile;
use core::sync::atomic::{AtomicU32, Ordering};

// ── Structures MADT ───────────────────────────────────────────────────────────

/// En-tête MADT (après SdtHeader)
#[repr(C, packed)]
struct MadtHeader {
    lapic_addr: u32,   // Adresse physique LAPIC (généralement 0xFEE00000)
    flags:      u32,   // bit 0 = dual 8259 present
}

/// Types d'entrées MADT
const MADT_TYPE_LAPIC:          u8 = 0;
const MADT_TYPE_IOAPIC:         u8 = 1;
const MADT_TYPE_INT_SRC_OVER:   u8 = 2;
const MADT_TYPE_NMI_SRC:        u8 = 3;
const MADT_TYPE_LAPIC_NMI:      u8 = 4;
const MADT_TYPE_LAPIC_ADDR_OVR: u8 = 5;
const MADT_TYPE_X2APIC:         u8 = 9;
const MADT_TYPE_X2APIC_NMI:     u8 = 10;

/// Entrée LAPIC (type 0) — CPU logique
#[repr(C, packed)]
struct MadtLapic {
    acpi_processor_id: u8,
    apic_id:           u8,
    flags:             u32,  // bit 0 = enabled, bit 1 = online capable
}

/// Entrée IOAPIC (type 1)
#[repr(C, packed)]
struct MadtIoApic {
    ioapic_id:  u8,
    _reserved:  u8,
    ioapic_addr:u32,
    gsi_base:   u32,
}

/// Interrupt Source Override (type 2) — remapping ISA IRQ → GSI
#[repr(C, packed)]
struct MadtIntSrcOvr {
    bus:    u8,    // Bus source (0 = ISA)
    source: u8,   // IRQ source (ISA IRQ 0–15)
    gsi:    u32,  // GSI destination
    flags:  u16,  // bit 0:1 = polarity, bit 2:3 = trigger mode
}

/// LAPIC Address Override (type 5) — 64-bit LAPIC address
#[repr(C, packed)]
struct MadtLapicAddrOvr {
    _reserved: u16,
    addr:      u64, // Nouvelle adresse physique LAPIC
}

/// Entrée x2APIC (type 9) — CPU avec UID > 255
#[repr(C, packed)]
struct MadtX2Apic {
    _reserved:        u16,
    x2apic_id:        u32,
    flags:            u32,
    acpi_processor_uid:u32,
}

// ── Résultats de parsing MADT ─────────────────────────────────────────────────

/// Informations extraites de la MADT
#[derive(Debug, Clone, Copy)]
pub struct MadtInfo {
    /// Nombre de CPUs logiques online
    pub cpu_count: u32,
    /// APIC IDs des CPUs (max 256)
    pub apic_ids:  [u32; 256],
    /// Adresse physique LAPIC (peut être override)
    pub lapic_phys: u64,
    /// Nombre d'I/O APICs
    pub ioapic_count: u32,
    /// GSI mappings pour les IRQ ISA (IRQ 0–15 → GSI)
    pub isa_irq_gsi:  [u32; 16],
    /// Flags de polarité/trigger ISA (1 bit polarity, 1 bit trigger par IRQ)
    pub isa_irq_flags:[u16; 16],
}

impl MadtInfo {
    const fn default() -> Self {
        Self {
            cpu_count: 0,
            apic_ids:  [0u32; 256],
            lapic_phys: 0xFEE0_0000,
            ioapic_count: 0,
            isa_irq_gsi:  [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15], // ISA: GSI=IRQ par défaut
            isa_irq_flags:[0u16; 16],
        }
    }
}

static MADT_CPU_COUNT: AtomicU32 = AtomicU32::new(0);

/// Retourne le nombre de CPUs détectés dans la MADT
pub fn madt_cpu_count() -> u32 {
    MADT_CPU_COUNT.load(Ordering::Relaxed)
}

// ── Parseur principal ─────────────────────────────────────────────────────────

/// Parse la MADT à partir de son adresse physique
///
/// Appelé par `init_acpi()` après avoir localisé la table.
pub fn parse_madt(madt_phys: u64) -> MadtInfo {
    let mut info = MadtInfo::default();
    if madt_phys == 0 || madt_phys >= 0x4000_0000 { return info; } // hors identity map

    use super::parser::SdtHeader;
    // SAFETY: adresse MADT validée par le parseur ACPI
    let header = unsafe { &*(madt_phys as *const SdtHeader) };
    let sig = unsafe { core::ptr::read_unaligned(&raw const (*header).signature) };
    if &sig != b"APIC" { return info; }

    let madt_len_raw = unsafe { core::ptr::read_unaligned(&raw const (*header).length) };
    let madt_len = madt_len_raw as usize;

    let madt_base_offset = core::mem::size_of::<SdtHeader>();
    // Sanity : besoin d'au moins SdtHeader + lapic_addr(4) + flags(4) = 44 octets
    if madt_len < madt_base_offset + 8 || madt_len > 65536 { return info; }

    // Lire LAPIC addr (offset 36, u32, potentiellement non-aligné)
    // read_unaligned obligatoire : Rust ≥1.82 vérifie l'alignement dans read_volatile
    let lapic_addr = unsafe {
        core::ptr::read_unaligned((madt_phys as usize + madt_base_offset) as *const u32)
    } as u64;
    info.lapic_phys = lapic_addr;

    // Itérer les entrées (offset 44 = SdtHeader(36) + lapic_addr(4) + flags(4))
    let mut offset = madt_base_offset + 8;

    while offset + 2 <= madt_len {
        // u8 reads — alignment 1, read_volatile OK ici
        let entry_type = unsafe { read_volatile((madt_phys as usize + offset) as *const u8) };
        let entry_len  = unsafe { read_volatile((madt_phys as usize + offset + 1) as *const u8) } as usize;
        if entry_len < 2 || offset + entry_len > madt_len { break; }

        let entry_base = madt_phys as usize + offset + 2;

        match entry_type {
            MADT_TYPE_LAPIC => {
                if entry_len < 2 + core::mem::size_of::<MadtLapic>() { offset += entry_len; continue; }
                // Lecture des champs packed via ptr::read pour éviter UB sur références non-alignées
                let base = entry_base as *const u8;
                let _acpi_proc_id = unsafe { *base };
                let apic_id       = unsafe { *base.add(1) };
                let flags         = unsafe { core::ptr::read_unaligned(base.add(2) as *const u32) };
                if flags & 3 != 0 && (info.cpu_count as usize) < 256 {
                    info.apic_ids[info.cpu_count as usize] = apic_id as u32;
                    info.cpu_count += 1;
                }
            }
            MADT_TYPE_IOAPIC => {
                if entry_len < 2 + core::mem::size_of::<MadtIoApic>() { offset += entry_len; continue; }
                let base = entry_base as *const u8;
                let ioapic_id   = unsafe { *base };
                let _reserved   = unsafe { *base.add(1) };
                let ioapic_addr = unsafe { core::ptr::read_unaligned(base.add(2) as *const u32) };
                let gsi_base    = unsafe { core::ptr::read_unaligned(base.add(6) as *const u32) };
                let _ = ioapic_id; let _ = _reserved;
                info.ioapic_count += 1;
                super::super::apic::io_apic::register_ioapic(ioapic_addr as u64, gsi_base);
            }
            MADT_TYPE_INT_SRC_OVER => {
                if entry_len < 2 + core::mem::size_of::<MadtIntSrcOvr>() { offset += entry_len; continue; }
                let base = entry_base as *const u8;
                let _bus   = unsafe { *base };
                let source = unsafe { *base.add(1) };
                let gsi    = unsafe { core::ptr::read_unaligned(base.add(2) as *const u32) };
                let flags  = unsafe { core::ptr::read_unaligned(base.add(6) as *const u16) };
                if source < 16 {
                    info.isa_irq_gsi[source as usize]   = gsi;
                    info.isa_irq_flags[source as usize] = flags;
                }
            }
            MADT_TYPE_LAPIC_ADDR_OVR => {
                if entry_len < 2 + core::mem::size_of::<MadtLapicAddrOvr>() { offset += entry_len; continue; }
                let base = entry_base as *const u8;
                let addr = unsafe { core::ptr::read_unaligned(base.add(2) as *const u64) };
                info.lapic_phys = addr;
                super::super::apic::local_apic::set_lapic_base(addr);
            }
            MADT_TYPE_X2APIC => {
                if entry_len < 2 + core::mem::size_of::<MadtX2Apic>() { offset += entry_len; continue; }
                let base = entry_base as *const u8;
                let _reserved   = unsafe { core::ptr::read_unaligned(base as *const u16) };
                let x2apic_id   = unsafe { core::ptr::read_unaligned(base.add(2) as *const u32) };
                let flags       = unsafe { core::ptr::read_unaligned(base.add(6) as *const u32) };
                if flags & 3 != 0 && (info.cpu_count as usize) < 256 {
                    info.apic_ids[info.cpu_count as usize] = x2apic_id;
                    info.cpu_count += 1;
                }
            }
            _ => {}
        }

        offset += entry_len;
    }

    MADT_CPU_COUNT.store(info.cpu_count, Ordering::Release);
    info
}
