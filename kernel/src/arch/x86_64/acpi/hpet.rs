//! # arch/x86_64/acpi/hpet.rs — High Precision Event Timer
//!
//! Implementation du HPET conformément à la table ACPI HPET.
//! Le HPET est utilisé comme source de temps de référence en attendant
//! la calibration TSC, ou comme fallback si le TSC n'est pas invariant.
//!
//! ## Registres HPET (MMIO)
//! - 0x000 : GCAP_ID — capabilities et fréquence (10 ns = 100 MHz si périodique)
//! - 0x010 : GEN_CFG — config globale (bit 0 = ENABLE_CNF, bit 1 = LEG_RT_CNF)
//! - 0x020 : GINTR_STA — statut des interruptions
//! - 0x0F0 : MAIN_CTR — compteur principal 64 bits
//! - 0x100+N*0x20 : Timer N comparateurs

#![allow(dead_code)]

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

// ── Offsets registres HPET ────────────────────────────────────────────────────

const HPET_GCAP_ID:   u64 = 0x000; // capabilities (64 bits)
const HPET_GEN_CFG:   u64 = 0x010; // config globale (64 bits)
const HPET_GINTR_STA: u64 = 0x020; // interrupt status
const HPET_MAIN_CTR:  u64 = 0x0F0; // compteur principal (64 bits)
const HPET_T0_CFG:    u64 = 0x100; // Timer 0 config (64 bits)
const HPET_T0_CMP:    u64 = 0x108; // Timer 0 comparateur (64 bits)

// Bits GEN_CFG
const HPET_ENABLE:    u64 = 1 << 0;
const HPET_LEG_RT:    u64 = 1 << 1; // Legacy Replacement Mapping

// GCAP_ID champs
const HPET_CLK_PERIOD_SHIFT: u32 = 32;  // bits 63:32 = période en femtosecondes
const HPET_NUM_TIMERS_SHIFT: u32 = 8;   // bits 12:8 = nombre de timers - 1

// ── État HPET ─────────────────────────────────────────────────────────────────

/// Informations HPET
#[derive(Debug, Clone, Copy)]
pub struct HpetInfo {
    pub mmio_base:      u64,   // Adresse MMIO physique
    pub clock_period_fs:u32,   // Période d'horloge en femtosecondes
    pub freq_hz:        u64,   // Fréquence en Hz
    pub n_timers:       u8,    // Nombre de timers (au moins 3)
    pub is_64bit:       bool,  // Compteur principal 64 bits
}

static HPET_BASE:      AtomicU64 = AtomicU64::new(0);
static HPET_PERIOD_FS: AtomicU32 = AtomicU32::new(0);
static HPET_FREQ_HZ:   AtomicU64 = AtomicU64::new(0);

// ── Structures table ACPI HPET ────────────────────────────────────────────────

/// Table ACPI HPET (après SdtHeader)
#[repr(C, packed)]
struct AcpiHpetTable {
    event_timer_block_id: u32,
    base_address:         [u8; 12], // Generic Address Structure (GAS)
    hpet_number:          u8,
    minimum_clock_tick:   u16,
    page_protection:      u8,
}

// ── Lecture / écriture HPET MMIO ─────────────────────────────────────────────

/// Lit un registre HPET 64 bits
#[inline]
fn hpet_read(offset: u64) -> u64 {
    let base = HPET_BASE.load(Ordering::Relaxed);
    // SAFETY: base HPET validée lors de l'init, offset registre connu
    unsafe { read_volatile((base + offset) as *const u64) }
}

/// Écrit un registre HPET 64 bits
#[inline]
fn hpet_write(offset: u64, val: u64) {
    let base = HPET_BASE.load(Ordering::Relaxed);
    // SAFETY: base HPET validée lors de l'init
    unsafe { write_volatile((base + offset) as *mut u64, val); }
}

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise le HPET depuis la table ACPI
///
/// Appelé par `init_acpi()` après avoir localisé la table HPET.
pub fn init_hpet(hpet_table_phys: u64) -> Option<HpetInfo> {
    if hpet_table_phys == 0 { return None; }

    use super::parser::SdtHeader;
    // SAFETY: adresse passée par le parseur ACPI
    let header = unsafe { &*(hpet_table_phys as *const SdtHeader) };
    if &header.signature != b"HPET" { return None; }

    // L'adresse MMIO est dans le GAS à offset 44 (SdtHeader=36 + block_id=4 + GAS...)
    // Structure GAS : address_space_id(1) + bit_width(1) + bit_offset(1) + access_size(1) + address(8)
    let gas_addr_offset = core::mem::size_of::<SdtHeader>() + 4 + 4; // +4 block_id, +4 padding GAS header
    // SAFETY: offset dans la table ACPI HPET
    let mmio_base = unsafe {
        read_volatile((hpet_table_phys as usize + gas_addr_offset) as *const u64)
    };

    HPET_BASE.store(mmio_base, Ordering::Release);

    // Lire GCAP_ID
    let gcap = hpet_read(HPET_GCAP_ID);
    let clock_period_fs = (gcap >> 32) as u32;
    if clock_period_fs == 0 { return None; }

    // Fréquence = 1e15 fs/s / période_fs
    let freq_hz: u64 = 1_000_000_000_000_000u64 / (clock_period_fs as u64);
    let n_timers = (((gcap >> 8) & 0x1F) as u8) + 1;
    let is_64bit = (gcap & (1 << 13)) != 0;

    HPET_PERIOD_FS.store(clock_period_fs, Ordering::Release);
    HPET_FREQ_HZ.store(freq_hz, Ordering::Release);

    // Désactiver le HPET avant configuration
    hpet_write(HPET_GEN_CFG, 0);

    // Réinitialiser le compteur principal
    hpet_write(HPET_MAIN_CTR, 0);

    // Activer sans Legacy Replacement (ne pas interférer avec les IRQ ISA)
    hpet_write(HPET_GEN_CFG, HPET_ENABLE);

    Some(HpetInfo {
        mmio_base,
        clock_period_fs,
        freq_hz,
        n_timers,
        is_64bit,
    })
}

// ── Primitives ────────────────────────────────────────────────────────────────

/// Lit le compteur principal HPET (64 bits)
#[inline]
pub fn hpet_read_counter() -> u64 {
    hpet_read(HPET_MAIN_CTR)
}

/// Convertit des µs en ticks HPET
pub fn hpet_us_to_ticks(us: u64) -> u64 {
    let freq = HPET_FREQ_HZ.load(Ordering::Relaxed);
    if freq == 0 { return 0; }
    us.saturating_mul(freq) / 1_000_000
}

/// Délai actif via HPET (en µs)
///
/// Usage : calibration dans les premières phases du boot avant TSC.
pub fn hpet_delay_us(us: u64) {
    let start = hpet_read_counter();
    let ticks = hpet_us_to_ticks(us);
    while hpet_read_counter().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

/// Fréquence HPET en Hz
pub fn hpet_freq_hz() -> u64 {
    HPET_FREQ_HZ.load(Ordering::Relaxed)
}

/// `true` si le HPET est disponible et initialisé
pub fn hpet_available() -> bool {
    HPET_BASE.load(Ordering::Relaxed) != 0 && HPET_FREQ_HZ.load(Ordering::Relaxed) != 0
}
