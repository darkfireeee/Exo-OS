// kernel/src/arch/x86_64/time/sources/hpet.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Source HPET — capacités, largeur compteur, guard overflow, delta safe
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Architecture HPET (High Precision Event Timer)
//   Spécification Intel IA-PC HPET Architecture Specification 1.0a
//   Base MMIO : lue depuis la table ACPI HPET (signature "HPET"), base_address.
//
//   Registres (offset depuis Base) :
//     0x000 : General Capabilities and ID Register (64-bit, RO)
//       [63:32] clock period en femtosecondes (COUNTER_CLK_PERIOD)
//       [31:16] Vendor ID
//       [12]    3-bit rev (bits 12..8 = REV_ID)
//       [13]    COUNT_SIZE_CAP : 1 = compteur 64-bit, 0 = compteur 32-bit
//       [7..5]  NUM_TIM_CAP : nombre de timers - 1
//       [0]     REV_ID (bits 7:0)
//     0x010 : General Configuration Register (64-bit, RW)
//       bit 0  = ENABLE_CNF (1 = compteur actif)
//       bit 1  = LEG_RT_CNF (Legacy Routing Config)
//     0x020 : General Interrupt Status Register (64-bit, RW)
//     0x0F0 : Main Counter Value Register (64-bit, RO — lis deux fois si 32-bit)
//     0x100 : Timer N Configuration Register (timers 0..31)
//
// ## Largeur du compteur (RÈGLE TIME-08)
//   COUNT_SIZE_CAP=1 → 64-bit : rollover toutes les ~42 milliards d'années.
//   COUNT_SIZE_CAP=0 → 32-bit : rollover toutes les 2^32 / freq ≈ 300 secondes
//     à 14.318 MHz (fréquence PIT-compatible HPET).
//   → Utiliser wrapping_sub() systématiquement pour les deux cas.
//   → Pour 32-bit, détecter un overflow (bit_64 XOR bit_32 read) et incrémenter
//     un compteur de wraps.
//
// ## Minimum Delta (Intel HPET spec §2.2)
//   La valeur de comparaison pour les one-shot timers doit être au moins
//   MIN_DELTA_TICKS > current counter + minimum_delta.
//   Typiquement ≥ 100 ticks HPET (≈ 7 µs à 14.318 MHz).
// ════════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU32, Ordering};
use super::ClockSource;
use crate::arch::x86_64::acpi::hpet as hpet_acpi;

// ── Offsets registres HPET ────────────────────────────────────────────────────

const HPET_REG_CAP_ID:    u64 = 0x000; // Capabilities and ID (RO)
#[allow(dead_code)]
const HPET_REG_CONFIG:    u64 = 0x010; // General Configuration (RW)
#[allow(dead_code)]
const HPET_REG_INT_STATUS:u64 = 0x020; // General Interrupt Status (RW)
const HPET_REG_COUNTER:   u64 = 0x0F0; // Main Counter Value (RO)

// ── Bits du registre CAP_ID ───────────────────────────────────────────────────

const CAP_COUNT_SIZE_CAP: u64 = 1 << 13; // 1 = 64-bit counter
const CAP_CLK_PERIOD_SHIFT: u32 = 32;    // bits 63:32 = clock period femtos
const CAP_NUM_TIMERS_MASK: u64 = 0x1F00; // bits 12:8 = NUM_TIM_CAP
const CAP_NUM_TIMERS_SHIFT: u32 = 8;
const CAP_VENDOR_ID_MASK:  u64 = 0xFFFF_0000; // bits 31:16
const CAP_VENDOR_ID_SHIFT: u32 = 16;
const CAP_REV_ID_MASK:     u64 = 0x00FF; // bits 7:0

// ── Constants ─────────────────────────────────────────────────────────────────

/// Période HPET minimum légale selon la spec : 100 femtosecondes (10 GHz max).
const HPET_MIN_PERIOD_FEMTOS: u64 = 100;
/// Période HPET maximum raisonnable : 100 ns = 100_000 femtos (10 MHz min).
const HPET_MAX_PERIOD_FEMTOS: u64 = 100_000_000;
/// Minimum delta pour les comparateurs HPET (ticks).
const HPET_MIN_DELTA_TICKS: u32 = 100;
/// Femtosecondes par seconde.
const FEMTOS_PER_SEC: u64 = 1_000_000_000_000_000;

// ── Capacités HPET ───────────────────────────────────────────────────────────

/// Capacités complètes du HPET détectées au boot.
#[derive(Debug, Clone, Copy)]
pub struct HpetCapabilities {
    /// `true` si le compteur est 64-bit, `false` si 32-bit.
    pub counter_64bit:     bool,
    /// Fréquence en Hz (= 10^15 / period_femtos).
    pub freq_hz:           u64,
    /// Période du compteur en femtosecondes (depuis registre CAP_ID bits 63:32).
    pub period_femtos:     u64,
    /// Nombre de timers disponibles (0-based dans CAP, affiché +1).
    pub num_timers:        u8,
    /// Vendor ID depuis le registre CAP.
    pub vendor_id:         u16,
    /// Revision ID.
    pub rev_id:            u8,
    /// Base MMIO adresse physique.
    pub base_addr:         u64,
}

impl HpetCapabilities {
    pub const fn unknown() -> Self {
        HpetCapabilities {
            counter_64bit: false,
            freq_hz: 0,
            period_femtos: 0,
            num_timers: 0,
            vendor_id: 0,
            rev_id: 0,
            base_addr: 0,
        }
    }

    /// Vérifie que les capacités sont valides.
    pub fn is_valid(&self) -> bool {
        self.period_femtos >= HPET_MIN_PERIOD_FEMTOS
            && self.period_femtos <= HPET_MAX_PERIOD_FEMTOS
            && self.freq_hz > 0
    }
}

// ── Globales HPET ─────────────────────────────────────────────────────────────

static HPET_IS_64BIT:     AtomicBool = AtomicBool::new(false);
static HPET_FREQ_HZ:      AtomicU64  = AtomicU64::new(0);
static HPET_BASE_ADDR:    AtomicU64  = AtomicU64::new(0);
/// Compteur d'overflows 32-bit (incrémenté à chaque rollover détecté).
static HPET_OVF_COUNT:    AtomicU64  = AtomicU64::new(0);
/// Dernière valeur 32-bit observée (pour détection de rollover).
static HPET_LAST_32:      AtomicU32  = AtomicU32::new(0);
static HPET_INIT_DONE:    AtomicBool = AtomicBool::new(false);

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise les capacités HPET depuis les registres MMIO.
/// À appeler une fois depuis `time_init()`, après `acpi::hpet::init()`.
pub fn init_hpet_source() {
    if HPET_INIT_DONE.swap(true, Ordering::Relaxed) { return; }

    let base = hpet_acpi::hpet_virt_base();
    if base == 0 { return; } // HPET non disponible.

    HPET_BASE_ADDR.store(base, Ordering::Relaxed);

    // Lire CAP_ID pour extraire la période et la largeur du compteur.
    let cap = mmio_read64(base + HPET_REG_CAP_ID);
    let period_femtos = cap >> CAP_CLK_PERIOD_SHIFT;
    let is_64bit = (cap & CAP_COUNT_SIZE_CAP) != 0;

    let freq_hz = if period_femtos > 0 {
        FEMTOS_PER_SEC / period_femtos
    } else {
        0
    };

    HPET_IS_64BIT.store(is_64bit, Ordering::Relaxed);
    HPET_FREQ_HZ.store(freq_hz,  Ordering::Relaxed);

    // Initialiser LAST_32 avec la valeur actuelle.
    if !is_64bit {
        let initial = mmio_read64(base + HPET_REG_COUNTER) as u32;
        HPET_LAST_32.store(initial, Ordering::Relaxed);
    }
}

/// Retourne les capacités HPET en lisant les globales.
pub fn hpet_capabilities() -> HpetCapabilities {
    let base = HPET_BASE_ADDR.load(Ordering::Relaxed);
    if base == 0 {
        return HpetCapabilities::unknown();
    }
    let cap = mmio_read64(base + HPET_REG_CAP_ID);
    let period_femtos = cap >> CAP_CLK_PERIOD_SHIFT;
    let num_timers = (((cap & CAP_NUM_TIMERS_MASK) >> CAP_NUM_TIMERS_SHIFT) + 1) as u8;
    let vendor_id  = ((cap & CAP_VENDOR_ID_MASK) >> CAP_VENDOR_ID_SHIFT) as u16;
    let rev_id     = (cap & CAP_REV_ID_MASK) as u8;
    let is_64bit   = (cap & CAP_COUNT_SIZE_CAP) != 0;
    let freq_hz    = HPET_FREQ_HZ.load(Ordering::Relaxed);

    HpetCapabilities {
        counter_64bit: is_64bit,
        freq_hz,
        period_femtos,
        num_timers,
        vendor_id,
        rev_id,
        base_addr: base,
    }
}

// ── Source HPET ClockSource ───────────────────────────────────────────────────

pub struct HpetSource;

impl ClockSource for HpetSource {
    fn name(&self) -> &'static str { "HPET" }
    fn rating(&self) -> u32 {
        if HPET_IS_64BIT.load(Ordering::Relaxed) { 320 } else { 300 }
    }

    fn read(&self) -> u64 {
        read_counter_extended()
    }

    fn freq_hz(&self) -> u64 {
        let hz = HPET_FREQ_HZ.load(Ordering::Relaxed);
        if hz > 0 { hz } else { hpet_acpi::hpet_freq_hz() }
    }

    fn available(&self) -> bool {
        hpet_acpi::hpet_available()
    }
}

// ── Lecture compteur ──────────────────────────────────────────────────────────

/// Lit le compteur HPET courant.
///
/// Pour HPET 32-bit : détecte les overflows et retourne un compteur étendu 64-bit.
/// Pour HPET 64-bit : lecture directe.
/// RÈGLE TIME-08 : toujours wrapping_sub pour les deltas.
#[inline]
pub fn read_counter_extended() -> u64 {
    let base = HPET_BASE_ADDR.load(Ordering::Relaxed);
    if base == 0 {
        return hpet_acpi::hpet_read_counter();
    }

    if HPET_IS_64BIT.load(Ordering::Relaxed) {
        // 64-bit : lecture directe, pas d'overflow possible.
        mmio_read64(base + HPET_REG_COUNTER)
    } else {
        // 32-bit : détecter rollover.
        read_32bit_extended(base)
    }
}

/// Lecture 32-bit avec gestion overflow.
/// Incrémente HPET_OVF_COUNT à chaque rollover et retourne un compteur 64-bit étendu.
fn read_32bit_extended(base: u64) -> u64 {
    let raw = mmio_read64(base + HPET_REG_COUNTER) as u32;
    let prev = HPET_LAST_32.load(Ordering::Relaxed);

    // Rollover détecté : raw < prev (le compteur a wrappé).
    if raw < prev {
        HPET_OVF_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    HPET_LAST_32.store(raw, Ordering::Relaxed);

    let ovf = HPET_OVF_COUNT.load(Ordering::Relaxed);
    // Reconstituer le compteur 64-bit : (overflows << 32) | raw.
    (ovf << 32) | raw as u64
}

// ── Primitives exposées ───────────────────────────────────────────────────────

/// Lit le compteur HPET courant (wrapper pour l'API publique).
#[inline(always)]
pub fn read() -> u64 {
    read_counter_extended()
}

/// Fréquence HPET en Hz.
#[inline(always)]
pub fn freq_hz() -> u64 {
    let hz = HPET_FREQ_HZ.load(Ordering::Relaxed);
    if hz > 0 { hz } else { hpet_acpi::hpet_freq_hz() }
}

/// `true` si le HPET est actif et initialisé.
#[inline(always)]
pub fn available() -> bool {
    hpet_acpi::hpet_available()
}

/// Delta de ticks HPET entre `start` et `now`, avec gestion du rollover.
/// RÈGLE TIME-08 : wrapping_sub obligatoire.
#[inline(always)]
pub fn delta(start: u64, now: u64) -> u64 {
    now.wrapping_sub(start)
}

/// Convertit des ticks HPET en nanosecondes.
/// Utilise la formule : ns = ticks × 10^6 / period_femtos.
/// (ticks / freq_hz × 10^9 = ticks × period_femtos / 10^6)
pub fn ticks_to_ns(ticks: u64) -> u64 {
    let period = {
        let base = HPET_BASE_ADDR.load(Ordering::Relaxed);
        if base == 0 { return 0; }
        let cap = mmio_read64(base + HPET_REG_CAP_ID);
        cap >> CAP_CLK_PERIOD_SHIFT
    };
    if period == 0 { return 0; }
    // ns = ticks × period_femtos / 10^6
    let ns = (ticks as u128).saturating_mul(period as u128) / 1_000_000;
    ns as u64
}

/// Convertit des nanosecondes en ticks HPET.
pub fn ns_to_ticks(ns: u64) -> u64 {
    let period = {
        let base = HPET_BASE_ADDR.load(Ordering::Relaxed);
        if base == 0 { return 0; }
        let cap = mmio_read64(base + HPET_REG_CAP_ID);
        cap >> CAP_CLK_PERIOD_SHIFT
    };
    if period == 0 { return 0; }
    (ns as u128 * 1_000_000 / period as u128) as u64
}

/// Attend `ns` nanosecondes en busy-waiting sur le compteur HPET.
/// RÈGLE CAL-CLI-01 : utilisé uniquement lors de la calibration (CLI actif ≤1ms).
pub fn hpet_wait_ns(ns: u64) {
    if !available() { return; }
    let ticks = ns_to_ticks(ns);
    let start = read();
    while delta(start, read()) < ticks {
        core::hint::spin_loop();
    }
}

/// Retourne le nombre d'overflows 32-bit détectés depuis le boot.
pub fn overflow_count() -> u64 {
    HPET_OVF_COUNT.load(Ordering::Relaxed)
}

/// Vérifie que le minimum delta est satisfait pour un comparateur HPET.
pub fn check_minimum_delta(current: u64, target: u64) -> bool {
    target.wrapping_sub(current) >= HPET_MIN_DELTA_TICKS as u64
}

// ── MMIO primitives ───────────────────────────────────────────────────────────

/// Lecture MMIO 64-bit non-cacheable (volatile read).
#[inline(always)]
fn mmio_read64(addr: u64) -> u64 {
    // SAFETY: addr est une adresse HPET MMIO valide, Uncacheable.
    unsafe {
        (addr as *const u64).read_volatile()
    }
}

/// Écriture MMIO 64-bit non-cacheable (volatile write).
#[inline(always)]
#[allow(dead_code)]
fn mmio_write64(addr: u64, val: u64) {
    unsafe {
        (addr as *mut u64).write_volatile(val)
    }
}
