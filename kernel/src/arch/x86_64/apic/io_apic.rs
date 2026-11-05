//! # arch/x86_64/apic/io_apic.rs — I/O APIC
//!
//! I/O APIC management for routing hardware IRQs to CPUs.
//!
//! ## Architecture
//! The I/O APIC receives hardware IRQs (8254 PIT timer, keyboard, disk, etc.)
//! and routes them to CPU LAPICs according to the Redirection Table (24 entries).
//!
//! ## Register Access (indirect via INDEX + DATA)
//! - INDEX (base + 0x00): register number
//! - DATA  (base + 0x10): 32-bit data

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default I/O APIC MMIO address
pub const IOAPIC_DEFAULT_BASE: u64 = 0xFEC0_0000;

/// IOAPIC registers
const IOAPIC_IOREGSEL: u32 = 0x00;
const IOAPIC_IOWIN: u32 = 0x10;

/// IOREGSEL register: internal register numbers
const IOAPIC_ID: u8 = 0x00;
const IOAPIC_VER: u8 = 0x01;
#[allow(dead_code)]
const IOAPIC_ARB: u8 = 0x02;
const IOAPIC_REDTBL: u8 = 0x10; // Start of redirection table (24 entries × 2 registers)

/// Redirection Table Entry bits
pub const IOAPIC_RTE_MASKED: u64 = 1 << 16;
pub const IOAPIC_RTE_LEVEL: u64 = 1 << 15;
pub const IOAPIC_RTE_ACTIVE_LO: u64 = 1 << 13;
pub const IOAPIC_RTE_REMOTE_IRR: u64 = 1 << 14;

// Delivery mode field (bits 10:8)
pub const IOAPIC_DM_FIXED: u64 = 0b000 << 8;
pub const IOAPIC_DM_LOWEST: u64 = 0b001 << 8;
pub const IOAPIC_DM_NMI: u64 = 0b100 << 8;
pub const IOAPIC_DM_INIT: u64 = 0b101 << 8;
pub const IOAPIC_DM_EXTINT: u64 = 0b111 << 8;

// Destination mode (bit 11)
pub const IOAPIC_DEST_PHYSICAL: u64 = 0;
pub const IOAPIC_DEST_LOGICAL: u64 = 1 << 11;

// ── IOAPIC Table (multi-APIC supported) ───────────────────────────────────────

const MAX_IOAPICS: usize = 8;

#[allow(dead_code)]
struct IoApicEntry {
    base: usize,
    gsi_base: u32,
    max_redir: u8,
}

// Static structure of up to 8 potential I/O APICs
static IOAPIC_BASES: [AtomicUsize; MAX_IOAPICS] = [const { AtomicUsize::new(0) }; MAX_IOAPICS];
static IOAPIC_GSI_BASE: [AtomicU8; MAX_IOAPICS] = [const { AtomicU8::new(0) }; MAX_IOAPICS];
static IOAPIC_COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// ── MMIO Read / Write ─────────────────────────────────────────────────────────

unsafe fn ioapic_read(base: usize, reg: u8) -> u32 {
    // SAFETY: caller guarantees valid base and reg
    write_volatile((base + IOAPIC_IOREGSEL as usize) as *mut u32, reg as u32);
    read_volatile((base + IOAPIC_IOWIN as usize) as *const u32)
}

unsafe fn ioapic_write(base: usize, reg: u8, val: u32) {
    // SAFETY: caller guarantees valid base and reg
    write_volatile((base + IOAPIC_IOREGSEL as usize) as *mut u32, reg as u32);
    write_volatile((base + IOAPIC_IOWIN as usize) as *mut u32, val);
}

// ── Registration from ACPI/MADT ──────────────────────────────────────────────

/// Registers an I/O APIC detected from ACPI MADT
///
/// Called by `acpi::madt::parse_ioapic()`.
pub fn register_ioapic(phys_base: u64, gsi_base: u32) {
    let idx = IOAPIC_COUNT.fetch_add(1, Ordering::AcqRel);
    if idx >= MAX_IOAPICS {
        return;
    }
    IOAPIC_BASES[idx].store(phys_base as usize, Ordering::Release);
    IOAPIC_GSI_BASE[idx].store(gsi_base as u8, Ordering::Release);
}

/// Index of the IOAPIC responsible for specified GSI
fn ioapic_for_gsi(gsi: u32) -> Option<(usize, u32)> {
    let count = IOAPIC_COUNT.load(Ordering::Acquire).min(MAX_IOAPICS);
    for i in (0..count).rev() {
        let gsi_base = IOAPIC_GSI_BASE[i].load(Ordering::Relaxed) as u32;
        let base = IOAPIC_BASES[i].load(Ordering::Relaxed);
        if base != 0 && gsi >= gsi_base {
            // SAFETY: base != 0 verified above; points to I/O APIC MMIO initialized by init_ioapic().
            let max_redir = unsafe { (ioapic_read(base, IOAPIC_VER) >> 16) & 0xFF };
            if gsi < gsi_base + max_redir + 1 {
                return Some((i, gsi - gsi_base));
            }
        }
    }
    None
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Reads full Redirection Table Entry (64-bit) for a GSI
pub fn read_rte(gsi: u32) -> Option<u64> {
    let (idx, local_gsi) = ioapic_for_gsi(gsi)?;
    let base = IOAPIC_BASES[idx].load(Ordering::Relaxed);
    let reg = IOAPIC_REDTBL + (local_gsi * 2) as u8;
    // SAFETY: base and reg validated by ioapic_for_gsi
    let lo = unsafe { ioapic_read(base, reg) } as u64;
    let hi = unsafe { ioapic_read(base, reg + 1) } as u64;
    Some(lo | (hi << 32))
}

/// Writes full Redirection Table Entry (64-bit) for a GSI
pub fn write_rte(gsi: u32, rte: u64) -> bool {
    let (idx, local_gsi) = match ioapic_for_gsi(gsi) {
        Some(v) => v,
        None => return false,
    };
    let base = IOAPIC_BASES[idx].load(Ordering::Relaxed);
    let reg = IOAPIC_REDTBL + (local_gsi * 2) as u8;
    // Write high part first to avoid premature delivery
    // SAFETY: base and reg validated
    unsafe {
        ioapic_write(base, reg + 1, (rte >> 32) as u32);
        ioapic_write(base, reg, rte as u32);
    }
    true
}

/// Configures a hardware IRQ: routes GSI to target LAPIC with given vector
///
/// - `gsi`: Global System Interrupt (number from ACPI)
/// - `vector`: x86_64 vector (32..255)
/// - `dest_apic`: target CPU LAPIC ID
/// - `active_low`: active-low polarity (ISA = false, PCI = true most often)
/// - `level`: level-triggered sensitivity (PCI = true, ISA = false)
pub fn route_irq(gsi: u32, vector: u8, dest_apic: u8, active_low: bool, level: bool) -> bool {
    assert!(
        !super::super::idt::is_exophoenix_reserved_vector(vector),
        "route_irq: vecteur {:#x} réservé ExoPhoenix (0xF0/0xF1/0xF2)",
        vector,
    );

    let mut rte: u64 = IOAPIC_DM_FIXED | IOAPIC_DEST_PHYSICAL | (vector as u64);
    if active_low {
        rte |= IOAPIC_RTE_ACTIVE_LO;
    }
    if level {
        rte |= IOAPIC_RTE_LEVEL;
    }
    rte |= (dest_apic as u64) << 56;
    write_rte(gsi, rte)
}

/// Masks a hardware IRQ (prevents delivery)
pub fn mask_irq(gsi: u32) {
    if let Some(rte) = read_rte(gsi) {
        write_rte(gsi, rte | IOAPIC_RTE_MASKED);
    }
}

/// Unmasks a hardware IRQ (re-enables delivery)
pub fn unmask_irq(gsi: u32) {
    if let Some(rte) = read_rte(gsi) {
        write_rte(gsi, rte & !IOAPIC_RTE_MASKED);
    }
}

/// Initializes all registered I/O APICs (masks all IRQs)
///
/// Called from `init_apic_system()` after ACPI detection.
pub fn init_all_ioapics() {
    let count = IOAPIC_COUNT.load(Ordering::Acquire).min(MAX_IOAPICS);
    for base_atomic in &IOAPIC_BASES[..count] {
        let base = base_atomic.load(Ordering::Relaxed);
        if base == 0 {
            continue;
        }

        // SAFETY: base validated during ACPI registration
        let max_redir = unsafe { (ioapic_read(base, IOAPIC_VER) >> 16) & 0xFF };
        for pin in 0..=max_redir {
            let reg = IOAPIC_REDTBL + (pin * 2) as u8;
            // SAFETY: register computed from valid max_redir
            unsafe {
                ioapic_write(base, reg, IOAPIC_RTE_MASKED as u32 | 0xFF);
                ioapic_write(base, reg + 1, 0);
            }
        }
    }
}

/// Reads ID of I/O APIC at index `idx`
pub fn ioapic_id(idx: usize) -> Option<u8> {
    if idx >= IOAPIC_COUNT.load(Ordering::Relaxed) {
        return None;
    }
    let base = IOAPIC_BASES[idx].load(Ordering::Relaxed);
    // SAFETY: base validated
    Some(unsafe { (ioapic_read(base, IOAPIC_ID) >> 24) as u8 })
}

/// Number of registered I/O APICs
pub fn ioapic_count() -> usize {
    IOAPIC_COUNT.load(Ordering::Relaxed)
}
