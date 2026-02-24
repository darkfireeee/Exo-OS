//! # arch/x86_64/apic/io_apic.rs — I/O APIC
//!
//! Gestion de l'I/O APIC pour le routage des IRQ hardware vers les CPU.
//!
//! ## Architecture
//! L'I/O APIC reçoit les IRQ hardware (timer PIT 8254, clavier, disque, etc.)
//! et les route vers les LAPIC des CPU selon la Redirection Table (24 entrées).
//!
//! ## Accès registres (indirect via INDEX + DATA)
//! - INDEX (base + 0x00) : numéro de registre
//! - DATA  (base + 0x10) : données 32 bits

#![allow(dead_code)]

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicUsize, AtomicU8, Ordering};

// ── Constantes ────────────────────────────────────────────────────────────────

/// Adresse MMIO par défaut de l'I/O APIC
pub const IOAPIC_DEFAULT_BASE: u64 = 0xFEC0_0000;

/// Registres IOAPIC
const IOAPIC_IOREGSEL: u32 = 0x00;
const IOAPIC_IOWIN:    u32 = 0x10;

/// Registre IOREGSEL : numéros de registres internes
const IOAPIC_ID:    u8 = 0x00;
const IOAPIC_VER:   u8 = 0x01;
const IOAPIC_ARB:   u8 = 0x02;
const IOAPIC_REDTBL:u8 = 0x10; // Début table de redirection (24 entrées × 2 registres)

/// Bits de la Redirection Table Entry
pub const IOAPIC_RTE_MASKED:    u64 = 1 << 16;
pub const IOAPIC_RTE_LEVEL:     u64 = 1 << 15;
pub const IOAPIC_RTE_ACTIVE_LO: u64 = 1 << 13;
pub const IOAPIC_RTE_REMOTE_IRR:u64 = 1 << 14;

// Champ mode de livraison (bits 10:8)
pub const IOAPIC_DM_FIXED:   u64 = 0b000 << 8;
pub const IOAPIC_DM_LOWEST:  u64 = 0b001 << 8;
pub const IOAPIC_DM_NMI:     u64 = 0b100 << 8;
pub const IOAPIC_DM_INIT:    u64 = 0b101 << 8;
pub const IOAPIC_DM_EXTINT:  u64 = 0b111 << 8;

// Mode de destination (bit 11)
pub const IOAPIC_DEST_PHYSICAL: u64 = 0;
pub const IOAPIC_DEST_LOGICAL:  u64 = 1 << 11;

// ── Table IOAPIC (multi-APIC supporté) ───────────────────────────────────────

const MAX_IOAPICS: usize = 8;

struct IoApicEntry {
    base:          usize,
    gsi_base:      u32,
    max_redir:     u8,
}

// Structure statique de 8 I/O APICs potentiels
static IOAPIC_BASES:    [AtomicUsize; MAX_IOAPICS] = {
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; MAX_IOAPICS]
};
static IOAPIC_GSI_BASE: [AtomicU8; MAX_IOAPICS] = {
    const ZERO: AtomicU8 = AtomicU8::new(0);
    [ZERO; MAX_IOAPICS]
};
static IOAPIC_COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// ── Lecture / écriture MMIO ───────────────────────────────────────────────────

unsafe fn ioapic_read(base: usize, reg: u8) -> u32 {
    // SAFETY: appelant garantit base et reg valides
    write_volatile((base + IOAPIC_IOREGSEL as usize) as *mut u32, reg as u32);
    read_volatile((base + IOAPIC_IOWIN as usize) as *const u32)
}

unsafe fn ioapic_write(base: usize, reg: u8, val: u32) {
    // SAFETY: appelant garantit base et reg valides
    write_volatile((base + IOAPIC_IOREGSEL as usize) as *mut u32, reg as u32);
    write_volatile((base + IOAPIC_IOWIN as usize) as *mut u32, val);
}

// ── Enregistrement depuis ACPI/MADT ──────────────────────────────────────────

/// Enregistre un I/O APIC détecté depuis le MADT ACPI
///
/// Appelé par `acpi::madt::parse_ioapic()`.
pub fn register_ioapic(phys_base: u64, gsi_base: u32) {
    let idx = IOAPIC_COUNT.fetch_add(1, Ordering::AcqRel);
    if idx >= MAX_IOAPICS { return; }
    IOAPIC_BASES[idx].store(phys_base as usize, Ordering::Release);
    IOAPIC_GSI_BASE[idx].store(gsi_base as u8, Ordering::Release);
}

/// Index de l'IOAPIC responsable du GSI spécifié
fn ioapic_for_gsi(gsi: u32) -> Option<(usize, u32)> {
    let count = IOAPIC_COUNT.load(Ordering::Acquire).min(MAX_IOAPICS);
    for i in (0..count).rev() {
        let gsi_base = IOAPIC_GSI_BASE[i].load(Ordering::Relaxed) as u32;
        let base = IOAPIC_BASES[i].load(Ordering::Relaxed);
        if base != 0 && gsi >= gsi_base {
            let max_redir = unsafe { (ioapic_read(base, IOAPIC_VER) >> 16) & 0xFF };
            if gsi < gsi_base + max_redir + 1 {
                return Some((i, gsi - gsi_base));
            }
        }
    }
    None
}

// ── API publique ──────────────────────────────────────────────────────────────

/// Lit la Redirection Table Entry complète (64 bits) pour un GSI
pub fn read_rte(gsi: u32) -> Option<u64> {
    let (idx, local_gsi) = ioapic_for_gsi(gsi)?;
    let base = IOAPIC_BASES[idx].load(Ordering::Relaxed);
    let reg = IOAPIC_REDTBL + (local_gsi * 2) as u8;
    // SAFETY: base et reg validés par ioapic_for_gsi
    let lo = unsafe { ioapic_read(base, reg) } as u64;
    let hi = unsafe { ioapic_read(base, reg + 1) } as u64;
    Some(lo | (hi << 32))
}

/// Écrit la Redirection Table Entry complète (64 bits) pour un GSI
pub fn write_rte(gsi: u32, rte: u64) -> bool {
    let (idx, local_gsi) = match ioapic_for_gsi(gsi) {
        Some(v) => v,
        None => return false,
    };
    let base = IOAPIC_BASES[idx].load(Ordering::Relaxed);
    let reg = IOAPIC_REDTBL + (local_gsi * 2) as u8;
    // Écrire d'abord la partie haute pour éviter une livraison prématurée
    // SAFETY: base et reg validés
    unsafe {
        ioapic_write(base, reg + 1, (rte >> 32) as u32);
        ioapic_write(base, reg,      rte as u32);
    }
    true
}

/// Configure une IRQ hardware : route le GSI vers un LAPIC cible avec le vecteur donné
///
/// - `gsi`        : Global System Interrupt (numéro depuis ACPI)
/// - `vector`     : vecteur x86_64 (32..255)
/// - `dest_apic`  : ID LAPIC du CPU destination
/// - `active_low` : polarité active-low (ISA = false, PCI = true le plus souvent)
/// - `level`      : sensibilité level (PCI = true, ISA = false)
pub fn route_irq(gsi: u32, vector: u8, dest_apic: u8, active_low: bool, level: bool) -> bool {
    let mut rte: u64 = IOAPIC_DM_FIXED | IOAPIC_DEST_PHYSICAL | (vector as u64);
    if active_low { rte |= IOAPIC_RTE_ACTIVE_LO; }
    if level      { rte |= IOAPIC_RTE_LEVEL; }
    rte |= (dest_apic as u64) << 56;
    write_rte(gsi, rte)
}

/// Masque une IRQ hardware (empêche la livraison)
pub fn mask_irq(gsi: u32) {
    if let Some(rte) = read_rte(gsi) {
        write_rte(gsi, rte | IOAPIC_RTE_MASKED);
    }
}

/// Démasque une IRQ hardware (réactive la livraison)
pub fn unmask_irq(gsi: u32) {
    if let Some(rte) = read_rte(gsi) {
        write_rte(gsi, rte & !IOAPIC_RTE_MASKED);
    }
}

/// Initialise tous les I/O APICs enregistrés (masque toutes les IRQ)
///
/// Appelé depuis `init_apic_system()` après la détection ACPI.
pub fn init_all_ioapics() {
    let count = IOAPIC_COUNT.load(Ordering::Acquire).min(MAX_IOAPICS);
    for i in 0..count {
        let base = IOAPIC_BASES[i].load(Ordering::Relaxed);
        if base == 0 { continue; }

        // SAFETY: base validée lors de l'enregistrement ACPI
        let max_redir = unsafe { (ioapic_read(base, IOAPIC_VER) >> 16) & 0xFF };
        for pin in 0..=(max_redir as u32) {
            let reg = IOAPIC_REDTBL + (pin * 2) as u8;
            // SAFETY: registre calculé depuis max_redir valide
            unsafe {
                ioapic_write(base, reg, IOAPIC_RTE_MASKED as u32 | 0xFF);
                ioapic_write(base, reg + 1, 0);
            }
        }
    }
}

/// Lit l'ID de l'I/O APIC d'index `idx`
pub fn ioapic_id(idx: usize) -> Option<u8> {
    if idx >= IOAPIC_COUNT.load(Ordering::Relaxed) { return None; }
    let base = IOAPIC_BASES[idx].load(Ordering::Relaxed);
    // SAFETY: base validée
    Some(unsafe { (ioapic_read(base, IOAPIC_ID) >> 24) as u8 })
}

/// Nombre d'I/O APICs enregistrés
pub fn ioapic_count() -> usize {
    IOAPIC_COUNT.load(Ordering::Relaxed)
}
