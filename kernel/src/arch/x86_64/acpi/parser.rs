//! # arch/x86_64/acpi/parser.rs — RSDP / XSDT / RSDT Parser
//!
//! Locates and parses root ACPI tables to discover
//! child tables (MADT, HPET, FADT, SRAT, etc.).
//!
//! ## Discovery Sequence
//! 1. Locate RSDP in EBDA segment (0xE0000–0xFFFFF)
//!    or passed by bootloader (Multiboot2 / UEFI)
//! 2. Read version: ACPI 1.0 → RSDT (32-bit), ACPI 2.0+ → XSDT (64-bit)
//! 3. Iterate entries to locate each table by 4-byte signature

use core::ptr::{read_unaligned, read_volatile};
use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::core::{layout, phys_to_virt, PhysAddr};

// ── ACPI Structures ───────────────────────────────────────────────────────────

/// Root System Description Pointer (RSDP) — ACPI 2.0+
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Rsdp {
    pub signature: [u8; 8], // "RSD PTR "
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,   // 0 = ACPI 1.0, 2 = ACPI 2.0+
    pub rsdt_addr: u32, // Physical address of RSDT (32-bit)
    // ACPI 2.0+ fields (revision >= 2)
    pub length: u32,
    pub xsdt_addr: u64, // Physical address of XSDT (64-bit)
    pub ext_checksum: u8,
    pub _reserved: [u8; 3],
}

/// System Description Table Header (common to all tables)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}

impl SdtHeader {
    pub fn signature_str(&self) -> &str {
        core::str::from_utf8(&self.signature).unwrap_or("????")
    }

    /// Validates checksum: sum of all bytes = 0 mod 256
    pub fn valid_checksum(&self) -> bool {
        let len = self.length as usize;
        // SAFETY: structure in ACPI memory, length validated by ACPI parser before any call.
        let bytes = unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, len) };
        bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b)) == 0
    }
}

// ── Global ACPI Information ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct AcpiInfo {
    pub acpi_version: u8,
    pub rsdp_phys: u64,
    pub xsdt_phys: u64,
    pub rsdt_phys: u32,
    pub madt_phys: u64,
    pub hpet_phys: u64,
    pub fadt_phys: u64,
    pub srat_phys: u64,
}

impl AcpiInfo {
    const fn zeroed() -> Self {
        Self {
            acpi_version: 0,
            rsdp_phys: 0,
            xsdt_phys: 0,
            rsdt_phys: 0,
            madt_phys: 0,
            hpet_phys: 0,
            fadt_phys: 0,
            srat_phys: 0,
        }
    }
}

static ACPI_INFO_ADDR: AtomicU64 = AtomicU64::new(0);
// Static storage for AcpiInfo (initialized once at boot)
struct AcpiInfoCell(core::cell::UnsafeCell<AcpiInfo>);
unsafe impl Sync for AcpiInfoCell {}

static ACPI_INFO: AcpiInfoCell = AcpiInfoCell(core::cell::UnsafeCell::new(AcpiInfo::zeroed()));

#[inline]
pub(crate) fn acpi_phys_accessible(phys: u64, len: usize) -> bool {
    if phys < 0x1000 {
        return false;
    }
    let Some(end) = phys.checked_add(len.max(1) as u64 - 1) else {
        return false;
    };
    end < layout::physmap_limit()
}

#[inline]
pub(crate) fn acpi_phys_ptr<T>(phys: u64) -> Option<*const T> {
    if !acpi_phys_accessible(phys, core::mem::size_of::<T>()) {
        return None;
    }
    let virt = if phys < crate::arch::constants::PHYSMAP_INITIAL_COVERAGE_BYTES {
        phys
    } else {
        phys_to_virt(PhysAddr::new(phys)).as_u64()
    };
    Some(virt as *const T)
}

#[inline]
pub(crate) fn acpi_read_unaligned<T: Copy>(phys: u64) -> Option<T> {
    let ptr = acpi_phys_ptr::<T>(phys)?;
    // SAFETY: `acpi_phys_ptr` verified that the full object is accessible.
    Some(unsafe { read_unaligned(ptr) })
}

/// Returns global AcpiInfo (valid after `init_acpi()`)
pub fn acpi_info() -> &'static AcpiInfo {
    // SAFETY: initialized before any use of ACPI structures
    unsafe { &*ACPI_INFO.0.get() }
}

/// Returns `true` if ACPI was successfully initialized
///
/// Checked via ACPI_INFO_ADDR != 0 (updated by `init_acpi_from_rsdp()`).
#[inline(always)]
pub fn acpi_available() -> bool {
    ACPI_INFO_ADDR.load(Ordering::Relaxed) != 0
}

// ── RSDP Location ─────────────────────────────────────────────────────────────

const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";

/// Searches for RSDP within a memory range (byte-by-byte scan, 16-byte aligned)
fn find_rsdp_in_range(start: u64, end: u64) -> Option<u64> {
    let mut addr = start & !15; // 16-byte aligned
    while addr + 20 <= end {
        let ptr = addr as *const [u8; 8];
        // SAFETY: address in Low Memory (EBDA / ROM BIOS) — identity mapped
        let sig = unsafe { read_volatile(ptr) };
        if &sig == RSDP_SIGNATURE {
            // SAFETY: addr is 16-byte aligned and in identity-mapped Low Memory;
            // signature was just verified, the Rsdp structure begins here.
            let _rsdp = unsafe { &*(addr as *const Rsdp) };
            // Validate checksum of first 20 bytes
            // SAFETY: same invariant — 20 B starting at `addr` are readable (addr+20 <= end checked).
            let bytes20 = unsafe { core::slice::from_raw_parts(addr as *const u8, 20) };
            let sum = bytes20.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            if sum == 0 {
                return Some(addr);
            }
        }
        addr += 16;
    }
    None
}

/// Locates RSDP in EBDA and BIOS ROM area
pub fn find_rsdp() -> Option<u64> {
    // 1. Search in EBDA (Extended BIOS Data Area)
    // SAFETY: Low Memory access — identity-mapped by boot
    let ebda_segment = unsafe { read_volatile(0x40E as *const u16) } as u64;
    let ebda_base = ebda_segment << 4;
    if (0x80000..0xA0000).contains(&ebda_base) {
        if let Some(a) = find_rsdp_in_range(ebda_base, ebda_base + 1024) {
            return Some(a);
        }
    }
    // 2. BIOS ROM area 0xE0000–0xFFFFF
    find_rsdp_in_range(0xE0000, 0x100000)
}

// ── Main Initialization ───────────────────────────────────────────────────────

/// Initializes ACPI from a known RSDP address (passed by bootloader)
///
/// Called from `boot::early_init` with the RSDP address provided by Multiboot2 / UEFI.
pub fn init_acpi_from_rsdp(rsdp_phys: u64) {
    if !acpi_phys_accessible(rsdp_phys, 20) {
        return;
    }
    // SAFETY: ACPI_INFO is a static UnsafeCell, initialized once during boot
    // (no reentrancy possible: called once before any SMP).
    let info = unsafe { &mut *ACPI_INFO.0.get() };
    info.rsdp_phys = rsdp_phys;

    let Some(revision) = acpi_read_unaligned::<u8>(rsdp_phys + 15) else {
        return;
    };
    let rsdt_addr = acpi_read_unaligned::<u32>(rsdp_phys + 16).unwrap_or(0);
    let xsdt_addr =
        if revision >= 2 && acpi_phys_accessible(rsdp_phys, core::mem::size_of::<Rsdp>()) {
            acpi_read_unaligned::<u64>(rsdp_phys + 24).unwrap_or(0)
        } else {
            0
        };
    info.acpi_version = revision;

    if revision >= 2 && xsdt_addr != 0 {
        info.xsdt_phys = xsdt_addr;
        parse_xsdt(xsdt_addr, info);
    } else if rsdt_addr != 0 {
        info.rsdt_phys = rsdt_addr;
        parse_rsdt(rsdt_addr as u64, info);
    }

    ACPI_INFO_ADDR.store(rsdp_phys, Ordering::Release);
}

/// Initializes ACPI via auto-detection (scan EBDA/ROM if no bootloader pointer)
pub fn init_acpi() -> Option<AcpiInfo> {
    let rsdp_addr = find_rsdp()?;
    init_acpi_from_rsdp(rsdp_addr);
    Some(*acpi_info())
}

// ── XSDT Parser ───────────────────────────────────────────────────────────────

fn parse_xsdt(xsdt_phys: u64, info: &mut AcpiInfo) {
    let Some(header) = acpi_read_unaligned::<SdtHeader>(xsdt_phys) else {
        return;
    };
    let length = header.length;
    let sig = header.signature;
    if &sig != b"XSDT" {
        return;
    }

    let header_size = core::mem::size_of::<SdtHeader>();
    if (length as usize) <= header_size || length > 4096 {
        return;
    }
    let n_entries = (length as usize - header_size) / 8;
    if n_entries > 64 {
        return;
    }
    let entries_base = xsdt_phys + header_size as u64;

    for i in 0..n_entries {
        let Some(entry_addr) = acpi_read_unaligned::<u64>(entries_base + (i * 8) as u64) else {
            break;
        };
        classify_table(entry_addr, info);
    }
}

fn parse_rsdt(rsdt_phys: u64, info: &mut AcpiInfo) {
    let Some(header) = acpi_read_unaligned::<SdtHeader>(rsdt_phys) else {
        return;
    };
    let length = header.length;
    let sig = header.signature;
    if &sig != b"RSDT" {
        return;
    }

    let header_size = core::mem::size_of::<SdtHeader>();
    // Guard against underflow (debug mode) and corrupted tables
    if (length as usize) <= header_size || length > 4096 {
        return;
    }
    let n_entries = (length as usize - header_size) / 4;
    if n_entries > 64 {
        return;
    }

    let entries_base = rsdt_phys + header_size as u64;

    for i in 0..n_entries {
        let Some(entry_addr32) = acpi_read_unaligned::<u32>(entries_base + (i * 4) as u64) else {
            break;
        };
        classify_table(entry_addr32 as u64, info);
    }
}

fn classify_table(phys: u64, info: &mut AcpiInfo) {
    let Some(sig) = acpi_read_unaligned::<[u8; 4]>(phys) else {
        return;
    };
    match &sig {
        b"APIC" => info.madt_phys = phys,
        b"HPET" => info.hpet_phys = phys,
        b"FACP" => info.fadt_phys = phys,
        b"SRAT" => info.srat_phys = phys,
        _ => {}
    }
}
