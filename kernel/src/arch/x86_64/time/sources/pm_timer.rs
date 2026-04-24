// kernel/src/arch/x86_64/time/sources/pm_timer.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Source PM Timer — ACPI Power Management Timer
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Architecture PM Timer
//   Spécifié par ACPI § 4.8.2 "Power Management Timer"
//   Port I/O : lu depuis la table ACPI FADT (champ PM_TMR_BLK ou X_PM_TMR_BLK).
//   Fréquence fixe : 3.579545 MHz (± 50 ppm selon fabricant de chipset).
//   Résolution : 24-bit ou 32-bit (bit TMR_VAL_EXT dans FADT flags).
//
// ## Modes de lecture
//   24-bit : rollover toutes les 2^24 / 3.579545 MHz ≈ 4.69 secondes.
//   32-bit : rollover toutes les 2^32 / 3.579545 MHz ≈ 1200 secondes (20 min).
//   → Utiliser wrapping_sub() + compteur d'overflow pour tracking long-terme.
//
// ## Anti-glitch (lecture triple)
//   Certains chipsets (ICH7, SB700) présentent des glitches sur une seule lecture.
//   La lecture doit être effectuée 3× et on prend le vote majoritaire (2/3).
//   Voir Linux kernel: arch/x86/kernel/time.c, read_pmtmr().
//
// ## Fréquence exacte
//   3_579_545 Hz (pas 3_580_000 — précis à 1 Hz près selon ACPI spec B.3).
//
// ## Calibration
//   Le PM Timer est la source de calibration de fallback quand HPET est absent.
//   Rating 200 (inférieur à HPET 300 mais bien supérieur à PIT 100).
//   Plus lent à lire que HPET (I/O port vs MMIO) mais toujours disponible.
// ════════════════════════════════════════════════════════════════════════════════

use super::ClockSource;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ── Constantes ────────────────────────────────────────────────────────────────

/// Fréquence PM Timer en Hz (ACPI spec, valeur exacte).
pub const PM_TIMER_FREQ_HZ: u64 = 3_579_545;
/// Fréquence en milliers de Hz pour calculs intermédiaires.
#[allow(dead_code)]
const PM_TIMER_FREQ_KHZ: u64 = 3_580; // arrondi pour guard

/// Masque valeur 24-bit.
const PM_TIMER_MASK_24: u32 = 0x00FF_FFFF;
/// Masque valeur 32-bit.
#[allow(dead_code)]
const PM_TIMER_MASK_32: u32 = 0xFFFF_FFFF;

/// Femtosecondes par tick PM Timer : 10^15 / 3_579_545 ≈ 279_365 fs/tick.
#[allow(dead_code)]
const PM_TIMER_FEMTOS_PER_TICK: u64 = 10_000_000_000_000_00 / PM_TIMER_FREQ_HZ;

// ── État PM Timer ─────────────────────────────────────────────────────────────

static PM_TIMER_PORT: AtomicU32 = AtomicU32::new(0);
/// `true` si le compteur est 32-bit (TMR_VAL_EXT bit dans FADT flags).
static PM_TIMER_IS_32BIT: AtomicBool = AtomicBool::new(false);
/// Compteur d'overflows 24-bit (incrémenté à chaque rollover x000000).
static PM_TIMER_OVF_COUNT: AtomicU64 = AtomicU64::new(0);
/// Dernière valeur 24-bit observée (pour détection rollover).
static PM_TIMER_LAST_24: AtomicU32 = AtomicU32::new(0);
/// Compteur d'overflows 32-bit.
static PM_TIMER_OVF_32_COUNT: AtomicU64 = AtomicU64::new(0);
/// Dernière valeur 32-bit (rollover guard).
static PM_TIMER_LAST_32: AtomicU32 = AtomicU32::new(0);
static PM_TIMER_INIT_DONE: AtomicBool = AtomicBool::new(false);
/// Nombre de glitches détectés (vote majoritaire échoué).
static PM_TIMER_GLITCH_COUNT: AtomicU64 = AtomicU64::new(0);

// ── Informations FADT ─────────────────────────────────────────────────────────

/// État détecté du PM Timer.
#[derive(Debug, Clone, Copy)]
pub struct PmTimerState {
    /// Port I/O en lecture.
    pub port: u16,
    /// Mode 32-bit si `true`, 24-bit si `false`.
    pub is_32bit: bool,
    /// Nombre d'overflows depuis le boot.
    pub ovf_count: u64,
    /// Nombre de glitches corrigés (lecture triple).
    pub glitch_count: u64,
    /// `true` si le port a été détecté (non nul).
    pub available: bool,
}

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise le PM Timer depuis la structure FADT ACPI.
/// Port et mode 24/32-bit proviennent de `acpi::fadt::pm_timer_port()`.
pub fn init_pm_timer(port: u16, is_32bit: bool) {
    if PM_TIMER_INIT_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    if port == 0 {
        return;
    }

    PM_TIMER_PORT.store(port as u32, Ordering::Relaxed);
    PM_TIMER_IS_32BIT.store(is_32bit, Ordering::Relaxed);

    // Initialiser les valeurs "last" pour la détection d'overflow.
    let initial = read_raw_single(port);
    if is_32bit {
        PM_TIMER_LAST_32.store(initial, Ordering::Relaxed);
    } else {
        PM_TIMER_LAST_24.store(initial & PM_TIMER_MASK_24, Ordering::Relaxed);
    }
}

/// Initialise le PM Timer en détectant automatiquement le port via les tables ACPI.
/// Fallback si `init_pm_timer()` n'est pas appelé directement.
pub fn auto_detect() -> bool {
    // Tenter de lire le port hardcodé pour les chipsets Intel ICH/PCH courants.
    // Le port 0x408 est le port PM Timer standard pour Intel PIIX4+.
    const PROBE_PORTS: [u16; 3] = [0x408, 0x808, 0xE308];
    for &port in &PROBE_PORTS {
        if probe_pm_timer(port) {
            init_pm_timer(port, false); // Démarrer en 24-bit par sécurité.
            return true;
        }
    }
    false
}

/// Teste si un port semble être un PM Timer valide.
/// Un PM Timer valide : la valeur ne doit pas être 0xFFFFFFFF et doit changer.
fn probe_pm_timer(port: u16) -> bool {
    let v1 = read_raw_single(port);
    if v1 == 0xFFFF_FFFF || v1 == 0 {
        return false;
    }
    // Attendre quelques ns (PIT busy-wait n'est pas disponible ici).
    for _ in 0..100 {
        core::hint::spin_loop();
    }
    let v2 = read_raw_single(port);
    // La valeur doit avoir changé (compteur qui avance).
    v2 != v1 && v2 != 0xFFFF_FFFF
}

// ── Source PM Timer ClockSource ───────────────────────────────────────────────

pub struct PmTimerSource;

impl ClockSource for PmTimerSource {
    fn name(&self) -> &'static str {
        "PM_TIMER"
    }
    fn rating(&self) -> u32 {
        200
    }

    fn read(&self) -> u64 {
        read_extended()
    }

    fn freq_hz(&self) -> u64 {
        PM_TIMER_FREQ_HZ
    }

    fn available(&self) -> bool {
        PM_TIMER_PORT.load(Ordering::Relaxed) != 0
    }
}

// ── Lecture PM Timer ──────────────────────────────────────────────────────────

/// Lecture PM Timer avec compteur étendu (gestion overflow 24/32-bit).
/// Retourne une valeur croissante monotone sur 64-bit.
pub fn read_extended() -> u64 {
    let port = PM_TIMER_PORT.load(Ordering::Relaxed) as u16;
    if port == 0 {
        return 0;
    }

    let is_32bit = PM_TIMER_IS_32BIT.load(Ordering::Relaxed);
    let raw = read_anti_glitch(port);

    if is_32bit {
        read_32bit_extended(raw)
    } else {
        read_24bit_extended(raw)
    }
}

/// Lecture brute avec vote majoritaire anti-glitch (3 lectures, on prend 2/3).
///
/// Certains chipsets produisent des valeurs erronées ponctuellement.
/// Voir Linux docs : Documentation/timers/hpet_and_acpi_pm_timer.txt
pub fn read_anti_glitch(port: u16) -> u32 {
    let a = read_raw_single(port);
    let b = read_raw_single(port);

    // Fast path : les deux premières sont identiques — cas normal.
    if a == b || a.wrapping_add(1) == b {
        return b;
    }

    // Troisième lecture pour vote majoritaire.
    let c = read_raw_single(port);
    PM_TIMER_GLITCH_COUNT.fetch_add(1, Ordering::Relaxed);

    // Retourner la valeur qui apparaît au moins 2 fois.
    if a == b {
        b
    } else if b == c {
        c
    } else if a == c {
        a
    } else {
        // Aucun accord — prendre la valeur médiane (compromis).
        let mut vals = [a, b, c];
        // Tri simple à 3 éléments.
        if vals[0] > vals[1] {
            vals.swap(0, 1);
        }
        if vals[1] > vals[2] {
            vals.swap(1, 2);
        }
        if vals[0] > vals[1] {
            vals.swap(0, 1);
        }
        vals[1]
    }
}

/// Lecture 24-bit avec overflow tracking.
fn read_24bit_extended(raw: u32) -> u64 {
    let val = raw & PM_TIMER_MASK_24;
    let prev = PM_TIMER_LAST_24.load(Ordering::Relaxed);

    if val < prev {
        // Overflow 24-bit détecté.
        PM_TIMER_OVF_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    PM_TIMER_LAST_24.store(val, Ordering::Relaxed);

    let ovf = PM_TIMER_OVF_COUNT.load(Ordering::Relaxed);
    (ovf << 24) | val as u64
}

/// Lecture 32-bit avec overflow tracking.
fn read_32bit_extended(raw: u32) -> u64 {
    let prev = PM_TIMER_LAST_32.load(Ordering::Relaxed);

    if raw < prev {
        PM_TIMER_OVF_32_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    PM_TIMER_LAST_32.store(raw, Ordering::Relaxed);

    let ovf = PM_TIMER_OVF_32_COUNT.load(Ordering::Relaxed);
    (ovf << 32) | raw as u64
}

/// Lecture I/O port directe (single read, sans anti-glitch).
#[inline(always)]
pub fn read_raw_single(port: u16) -> u32 {
    let val: u32;
    // SAFETY: Port I/O PM Timer — lecture 32-bit, CPL=0 requis.
    unsafe {
        core::arch::asm!(
            "in eax, dx",
            in("dx")  port,
            out("eax") val,
            options(nostack, nomem)
        );
    }
    val
}

// ── Primitives publiques ──────────────────────────────────────────────────────

/// Lit le compteur PM Timer courant (valeur brute, 24 ou 32-bit).
#[inline(always)]
pub fn read() -> u64 {
    read_extended()
}

/// Fréquence PM Timer en Hz.
#[inline(always)]
pub fn freq_hz() -> u64 {
    PM_TIMER_FREQ_HZ
}

/// `true` si le PM Timer est disponible et initialisé.
#[inline(always)]
pub fn available() -> bool {
    PM_TIMER_PORT.load(Ordering::Relaxed) != 0
}

/// Delta de ticks PM Timer entre `start` et `now`.
/// Fonctionne même à travers les overflows grâce à wrapping_sub.
#[inline(always)]
pub fn delta(start: u64, now: u64) -> u64 {
    now.wrapping_sub(start)
}

/// Convertit des ticks PM Timer en nanosecondes.
/// ns = ticks × 10^9 / 3_579_545
pub fn ticks_to_ns(ticks: u64) -> u64 {
    // Éviter l'overflow : ticks × 10^9 peut dépasser u64 pour grands ticks.
    let ns = (ticks as u128).saturating_mul(1_000_000_000) / PM_TIMER_FREQ_HZ as u128;
    ns as u64
}

/// Convertit des nanosecondes en ticks PM Timer.
pub fn ns_to_ticks(ns: u64) -> u64 {
    let ticks = (ns as u128).saturating_mul(PM_TIMER_FREQ_HZ as u128) / 1_000_000_000;
    ticks as u64
}

/// Attend `ns` nanosecondes en busy-waiting sur le PM Timer.
/// RÈGLE CAL-CLI-01 : utiliser uniquement lors de la calibration (CLI actif ≤1ms).
pub fn pm_timer_wait_ns(ns: u64) {
    if !available() {
        return;
    }
    let ticks = ns_to_ticks(ns);
    let start = read();
    while delta(start, read()) < ticks {
        core::hint::spin_loop();
    }
}

/// Retourne l'état complet du PM Timer pour diagnostics.
pub fn pm_timer_state() -> PmTimerState {
    let port = PM_TIMER_PORT.load(Ordering::Relaxed) as u16;
    let is_32bit = PM_TIMER_IS_32BIT.load(Ordering::Relaxed);
    let ovf = if is_32bit {
        PM_TIMER_OVF_32_COUNT.load(Ordering::Relaxed)
    } else {
        PM_TIMER_OVF_COUNT.load(Ordering::Relaxed)
    };
    PmTimerState {
        port,
        is_32bit,
        ovf_count: ovf,
        glitch_count: PM_TIMER_GLITCH_COUNT.load(Ordering::Relaxed),
        available: port != 0,
    }
}

/// Retourne le nombre de glitches corrigés (anti-glitch vote).
pub fn glitch_count() -> u64 {
    PM_TIMER_GLITCH_COUNT.load(Ordering::Relaxed)
}

/// Retourne le nombre d'overflows détectés.
pub fn overflow_count() -> u64 {
    if PM_TIMER_IS_32BIT.load(Ordering::Relaxed) {
        PM_TIMER_OVF_32_COUNT.load(Ordering::Relaxed)
    } else {
        PM_TIMER_OVF_COUNT.load(Ordering::Relaxed)
    }
}
