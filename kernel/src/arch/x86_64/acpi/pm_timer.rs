//! # arch/x86_64/acpi/pm_timer.rs — ACPI Power Management Timer
//!
//! Le PM Timer est un timer hardware à 3.579545 MHz accessible via I/O ports.
//! Il est utilisé en dernier recours pour la calibration TSC quand ni le PIT
//! ni le HPET ne sont disponibles (laptops modernes, UEFI).
//!
//! ## Accès
//! Via un port I/O 32 bits (PMTMR_BLKX dans la FADT, offset variable).
//! Compteur 24 bits ou 32 bits selon le chipset (bit 8 du FADT flags).

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};

/// Fréquence du PM Timer : 3.579545 MHz exactement
pub const PM_TIMER_FREQ_HZ: u32 = 3_579_545;

/// Masque 24 bits (compteur débordant à 0xFFFFFF si non 32 bits)
const PM_TMR_MASK_24: u32 = 0x00FF_FFFF;
const PM_TMR_MASK_32: u32 = 0xFFFF_FFFF;

static PM_TMR_PORT:  AtomicU32  = AtomicU32::new(0);
static PM_TMR_32BIT: AtomicBool = AtomicBool::new(false);

// ── Structure FADT (minimal) ──────────────────────────────────────────────────

/// FADT (Fixed ACPI Description Table) — champs minimaux pour pm_timer_blk
#[repr(C, packed)]
struct FadtMinimal {
    // SdtHeader (36 octets) + champs FADT
    // _header (36 bytes) — on lit directement après le header
    firmware_ctrl:     u32,  // +36
    dsdt_addr:         u32,  // +40
    _reserved1:        u8,   // +44
    preferred_pm_profile: u8,// +45
    sci_int:           u16,  // +46
    smi_cmd:           u32,  // +48
    acpi_enable:       u8,   // +52
    acpi_disable:      u8,   // +53
    s4bios_req:        u8,   // +54
    pstate_cnt:        u8,   // +55
    pm1a_evt_blk:      u32,  // +56
    pm1b_evt_blk:      u32,  // +60
    pm1a_ctl_blk:      u32,  // +64
    pm1b_ctl_blk:      u32,  // +68
    pm2_ctl_blk:       u32,  // +72
    pm_tmr_blk:        u32,  // +76 ← port I/O PM Timer
    gpe0_blk:          u32,  // +80
    gpe1_blk:          u32,  // +84
    pm1_evt_len:       u8,   // +88
    pm1_ctl_len:       u8,   // +89
    pm2_ctl_len:       u8,   // +90
    pm_tmr_len:        u8,   // +91 (doit être 4)
    gpe0_blk_len:      u8,   // +92
    gpe1_blk_len:      u8,   // +93
    gpe1_base:         u8,   // +94
    cst_cnt:           u8,   // +95
    p_lvl2_lat:        u16,  // +96
    p_lvl3_lat:        u16,  // +98
    flush_size:        u16,  // +100
    flush_stride:      u16,  // +102
    duty_offset:       u8,   // +104
    duty_width:        u8,   // +105
    day_alarm:         u8,   // +106
    mon_alarm:         u8,   // +107
    century:           u8,   // +108
    iapc_boot_arch:    u16,  // +109
    _reserved2:        u8,   // +111
    flags:             u32,  // +112 — bit 8 = TMR_VAL_EXT (32 bits)
}

const FADT_HEADER_SIZE: usize = 36;
const FADT_PM_TMR_BLK_OFF: usize = FADT_HEADER_SIZE + 40; // offset 76 depuis début FADT
const FADT_FLAGS_OFF:      usize = FADT_HEADER_SIZE + 76; // offset 112
const FADT_TMR_VAL_EXT:    u32   = 1 << 8;

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise le PM Timer depuis la table FADT
///
/// Appelé par `init_acpi()` après localisation de la table FACP/FADT.
pub fn init_pm_timer(fadt_phys: u64) -> bool {
    if fadt_phys == 0 { return false; }

    if fadt_phys >= 0x4000_0000 { return false; } // hors identity map
    // read_unaligned : table FADT potentiellement non-alignée sur 4 octets
    let pm_tmr_blk = unsafe {
        core::ptr::read_unaligned((fadt_phys as usize + FADT_PM_TMR_BLK_OFF) as *const u32)
    };
    if pm_tmr_blk == 0 { return false; }

    let fadt_flags = unsafe {
        core::ptr::read_unaligned((fadt_phys as usize + FADT_FLAGS_OFF) as *const u32)
    };

    PM_TMR_PORT.store(pm_tmr_blk, Ordering::Release);
    PM_TMR_32BIT.store((fadt_flags & FADT_TMR_VAL_EXT) != 0, Ordering::Release);
    true
}

// ── Lecture du Timer ──────────────────────────────────────────────────────────

/// Lit la valeur courante du PM Timer
#[inline]
pub fn pm_timer_read() -> u32 {
    let port = PM_TMR_PORT.load(Ordering::Relaxed) as u16;
    if port == 0 { return 0; }

    // SAFETY: port I/O PM Timer validé à l'init
    let val = unsafe { super::super::inl(port) };
    if PM_TMR_32BIT.load(Ordering::Relaxed) {
        val & PM_TMR_MASK_32
    } else {
        val & PM_TMR_MASK_24
    }
}

/// Lit le PM Timer avec gestion du wrap (compteur 24 bits)
pub fn pm_timer_read_wrapped() -> u64 {
    static WRAP_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    static LAST_VAL: AtomicU32 = AtomicU32::new(0);

    let current = pm_timer_read();
    let last = LAST_VAL.load(Ordering::Relaxed);

    if current < last {
        WRAP_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    LAST_VAL.store(current, Ordering::Release);

    let wraps = WRAP_COUNT.load(Ordering::Relaxed);
    let mask = if PM_TMR_32BIT.load(Ordering::Relaxed) { PM_TMR_MASK_32 } else { PM_TMR_MASK_24 };
    wraps * (mask as u64 + 1) + current as u64
}

/// Délai actif via PM Timer (en millisecondes)
///
/// Précis à ±1 tick (~280ns à 3.58 MHz).
pub fn pm_timer_delay_ms(ms: u32) {
    if PM_TMR_PORT.load(Ordering::Relaxed) == 0 { return; }

    let ticks_per_ms = PM_TIMER_FREQ_HZ / 1000;
    let total_ticks  = (ms as u64) * (ticks_per_ms as u64);

    let mask = if PM_TMR_32BIT.load(Ordering::Relaxed) {
        PM_TMR_MASK_32 as u64
    } else {
        PM_TMR_MASK_24 as u64
    };

    let start = pm_timer_read() as u64;
    let mut elapsed: u64 = 0;
    let mut prev = start;

    while elapsed < total_ticks {
        core::hint::spin_loop();
        let cur = pm_timer_read() as u64;
        let delta = if cur >= prev { cur - prev } else { mask + 1 - prev + cur };
        elapsed += delta;
        prev = cur;
    }
}

/// Retourne une mesure de ms depuis le boot (via PM Timer)
pub fn pm_timer_read_ms() -> u64 {
    let ticks = pm_timer_read_wrapped();
    ticks * 1000 / PM_TIMER_FREQ_HZ as u64
}

/// `true` si le PM Timer est disponible
pub fn pm_timer_available() -> bool {
    PM_TMR_PORT.load(Ordering::Relaxed) != 0
}

/// Retourne `true` si le PM Timer est en mode 32 bits (sinon 24 bits).
#[inline]
pub fn pm_timer_is_32bit() -> bool {
    PM_TMR_32BIT.load(Ordering::Relaxed)
}

// ── Port I/O (accès depuis pm_timer) ─────────────────────────────────────────
// Petit shim inline plutôt que d'appeler super::super::x86_64::inl directement

use super::super::inl;
