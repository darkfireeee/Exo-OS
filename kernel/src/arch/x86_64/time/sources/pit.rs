// kernel/src/arch/x86_64/time/sources/pit.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Source PIT — Programmable Interval Timer 8254 (fallback ultime)
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Architecture PIT (Intel 8254)
//   Fréquence d'entrée : 14.31818 MHz / 12 = 1.193182 MHz.
//   3 canaux :
//     Canal 0 (0x40) : générateur d'IRQ0 (timer système). Mode 3 par défaut.
//     Canal 1 (0x41) : DRAM refresh (obsolète, ignoré).
//     Canal 2 (0x42) : PC speaker / calibration TSC.
//   Port de contrôle : 0x43 (OCW).
//   Port gate/status  : 0x61 (bits 0 et 1 contrôlent canal 2, bit 5 = OUT2).
//
// ## Modes d'opération (canal 2 pour calibration)
//   Mode 0 (One-Shot) : compte de N à 0, OUTPUT passe haut quand done.
//     → Utilisé pour la calibration TSC : attendre bit 5 du port 0x61.
//   Mode 2 (Rate Generator) : recharge automatique, périodique.
//   Mode 3 (Square Wave) : carré symétrique (mode par défaut du canal 0).
//
// ## Commande OCW (0x43)
//   Bits [7:6] : canal (00=0, 01=1, 10=2, 11=readback)
//   Bits [5:4] : mode accès (01=LSB, 10=MSB, 11=LSB puis MSB)
//   Bits [3:1] : mode opération (000=0, 001=1, 010=2, 011=3, 100=4, 101=5)
//   Bit  [0]   : format (0=binaire, 1=BCD)
//
//   Exemple : 0xB0 = 1011_0000 = canal 2, LSB+MSB, mode 0, binaire
//             0x34 = 0011_0100 = canal 0, LSB+MSB, mode 2, binaire
//
// ## Détection QEMU/KVM (RÈGLE PIT-QEMU-01)
//   Sur QEMU TCG (émulation pure), le PIT est émulé mais peut avancer très lentement
//   selon le host. Sur QEMU KVM, il est émulé par le kernel → timing correct.
//   Détection : comparer un busy-wait très court (1000 ticks PIT) avec la durée
//   TSC mesurée. Si TSC n'a pas avancé après 100 µs équivalent → QEMU TCG suspecté.
//
// ## Qualité de calibration
//   Le PIT est peu précis (fréquence ±50 ppm) et le busy-wait est perturbé par
//   les IRQs si les interruptions ne sont pas masquées (CLI).
//   Rating 50 (minimum acceptable pour la calibration de fallback).
//   Si le PIT ne répond pas après 200M iters → retourner None (QEMU TCG).
// ════════════════════════════════════════════════════════════════════════════════


use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use super::ClockSource;

// ── Ports PIT ─────────────────────────────────────────────────────────────────

const PIT_CHANNEL0: u16 = 0x40; // Canal 0 (timer système, IRQ0)
#[allow(dead_code)]
const PIT_CHANNEL1: u16 = 0x41; // Canal 1 (DRAM refresh, ignoré)
const PIT_CHANNEL2: u16 = 0x42; // Canal 2 (speaker / calibration)
const PIT_CONTROL:  u16 = 0x43; // Register de contrôle (OCW)
const PIT_GATE:     u16 = 0x61; // Port speaker/gate pour canal 2

// ── Bits du port 0x61 ─────────────────────────────────────────────────────────

const PIT_GATE61_GATE2:   u8 = 1 << 0; // 1 = Gate canal 2 activé
const PIT_GATE61_SPKR:    u8 = 1 << 1; // 1 = Canal 2 connecté au speaker
const PIT_GATE61_OUT2:    u8 = 1 << 5; // 1 = Sortie canal 2 à HIGH (fin one-shot)

// ── Commandes OCW ─────────────────────────────────────────────────────────────

/// Canal 2, LSB+MSB, Mode 0 (one-shot), Binaire.
const PIT_OCW_CH2_MODE0: u8 = 0xB0;
/// Canal 0, LSB+MSB, Mode 2 (rate generator), Binaire.
const PIT_OCW_CH0_MODE2: u8 = 0x34;
/// Canal 2, Latch commande (pour lire la valeur courante).
const PIT_OCW_CH2_LATCH: u8 = 0x80;

// ── Constantes de calibration ─────────────────────────────────────────────────

/// Fréquence PIT en Hz (fixe par spec ISA 8254).
pub const PIT_FREQ_HZ: u64 = 1_193_182;
/// Ticks PIT pour ≈10ms de mesure (PIT_FREQ_HZ / 100 = 11931).
const PIT_COUNT_10MS: u16 = 11_931;
/// Timeout max pour attendre la fin du one-shot (iterations spin_loop).
#[allow(dead_code)]
const PIT_MAX_ITER: u32 = 200_000_000;

// ── État PIT ──────────────────────────────────────────────────────────────────

/// État de la dernière calibration PIT.
#[derive(Debug, Clone, Copy)]
pub struct PitCalibrationResult {
    /// Fréquence TSC estimée via PIT (Hz).
    pub tsc_hz:       u64,
    /// Ticks TSC mesurés pendant PIT_COUNT_10MS ticks PIT.
    pub tsc_delta:    u64,
    /// `true` si la mesure est dans les limites de confiance.
    pub valid:        bool,
    /// Indicateur de qualité : 0 (invalide) à 100 (parfait).
    pub quality:      u8,
    /// QEMU TCG suspecté (PIT bloqué ou drift anormal).
    pub qemu_tcg_suspect: bool,
}

// ── Globales ──────────────────────────────────────────────────────────────────

static PIT_QEMU_TCG_DETECTED: AtomicBool = AtomicBool::new(false);
static PIT_LAST_QUALITY:      AtomicU32  = AtomicU32::new(0);
static PIT_CALIBRATION_COUNT: AtomicU64  = AtomicU64::new(0);

// ── Source PIT ClockSource ────────────────────────────────────────────────────

pub struct PitSource;

impl ClockSource for PitSource {
    fn name(&self) -> &'static str { "PIT" }
    fn rating(&self) -> u32 {
        // Rating réduit à 30 si QEMU TCG détecté (signal non fiable).
        if PIT_QEMU_TCG_DETECTED.load(Ordering::Relaxed) { 30 } else { 50 }
    }

    fn read(&self) -> u64 {
        // Latch canal 2, lire LSB+MSB (mode compteur décrémenté).
        read_latch_ch2() as u64
    }

    fn freq_hz(&self) -> u64 { PIT_FREQ_HZ }

    fn available(&self) -> bool {
        !PIT_QEMU_TCG_DETECTED.load(Ordering::Relaxed)
    }
}

// ── Lecture PIT ───────────────────────────────────────────────────────────────

/// Lit la valeur latched du canal 2 (count courant).
/// Envoie la commande de latch puis lit LSB+MSB.
pub fn read_latch_ch2() -> u16 {
    unsafe {
        outb_raw(PIT_CONTROL, PIT_OCW_CH2_LATCH);
        io_delay();
        let lo = inb_raw(PIT_CHANNEL2);
        io_delay();
        let hi = inb_raw(PIT_CHANNEL2);
        ((hi as u16) << 8) | lo as u16
    }
}

/// Lit la valeur du canal 2 sans latch (lecture en cours d'opération).
/// Peut produire des valeurs instables si le compteur se modifie entre les lectures.
pub fn read_ch2_raw() -> u16 {
    unsafe {
        let lo = inb_raw(PIT_CHANNEL2);
        let hi = inb_raw(PIT_CHANNEL2);
        ((hi as u16) << 8) | lo as u16
    }
}

// ── Configuration PIT ─────────────────────────────────────────────────────────

/// Configure le canal 2 en mode 0 (one-shot) avec la durée donnée.
/// Le canal 2 est utilisé pour la calibration TSC (pas d'IRQ — lecture polling).
///
/// # Sécurité
/// Cette fonction modifie les ports I/O PIT. Ne pas appeler depuis un contexte
/// où les interruptions sont actives (CLI requis avant calibration).
pub fn setup_ch2_oneshot(count: u16) {
    unsafe {
        // Désactiver gate canal 2 pendant la configuration.
        let gate = inb_raw(PIT_GATE);
        outb_raw(PIT_GATE, gate & !(PIT_GATE61_GATE2 | PIT_GATE61_SPKR));
        io_delay();

        // Configurer canal 2 : mode 0, LSB+MSB, binaire.
        outb_raw(PIT_CONTROL, PIT_OCW_CH2_MODE0);
        io_delay();

        // Charger le compteur LSB puis MSB.
        outb_raw(PIT_CHANNEL2, (count & 0xFF) as u8);
        io_delay();
        outb_raw(PIT_CHANNEL2, (count >> 8) as u8);
        io_delay();

        // Activer gate canal 2 (démarre le comptage).
        let gate2 = inb_raw(PIT_GATE);
        outb_raw(PIT_GATE, (gate2 | PIT_GATE61_GATE2) & !PIT_GATE61_SPKR);
        io_delay();
    }
}

/// Attend la fin du one-shot canal 2 (bit 5 du port 0x61 = OUT2 = HIGH).
/// Retourne `true` si la fin est détectée, `false` si timeout (QEMU TCG).
///
/// Timeout : fenêtre temporelle bornée à ~20 ms via le TSC.
pub fn wait_ch2_done() -> bool {
    let timeout_cycles = crate::arch::x86_64::cpu::tsc::tsc_us_to_cycles(20_000)
        .max(20_000_000);
    let start_tsc = crate::arch::x86_64::cpu::tsc::read_tsc();

    loop {
        let val = unsafe { inb_raw(PIT_GATE) };
        if val & PIT_GATE61_OUT2 != 0 {
            return true;
        }
        if crate::arch::x86_64::cpu::tsc::read_tsc().wrapping_sub(start_tsc) >= timeout_cycles {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Désactive le canal 2 (gate bas + speaker muet).
pub fn disable_ch2() {
    unsafe {
        let gate = inb_raw(PIT_GATE);
        outb_raw(PIT_GATE, gate & !(PIT_GATE61_GATE2 | PIT_GATE61_SPKR));
    }
}

/// Configure le canal 0 en mode 2 (rate generator) avec le diviseur donné.
/// Diviseur 0 = 65536 (max, ≈ 18.2 Hz tick rate).
pub fn setup_ch0_rate(divisor: u16) {
    unsafe {
        outb_raw(PIT_CONTROL, PIT_OCW_CH0_MODE2);
        io_delay();
        outb_raw(PIT_CHANNEL0, (divisor & 0xFF) as u8);
        io_delay();
        outb_raw(PIT_CHANNEL0, (divisor >> 8) as u8);
        io_delay();
    }
}

// ── Calibration ───────────────────────────────────────────────────────────────

/// Calibre la fréquence TSC en utilisant le PIT canal 2.
///
/// Procédure :
///   1. CLI (appelant doit avoir désactivé les interruptions)
///   2. Configurer canal 2 en one-shot ~10ms (11931 ticks)
///   3. Lire TSC de départ (LFENCE + RDTSC)
///   4. Attendre OUT2 = HIGH ou timeout 200M iters
///   5. Lire TSC de fin (RDTSCP + LFENCE)
///   6. Calculer fréquence TSC = delta_tsc × PIT_FREQ_HZ / 11931
///
/// Retourne `None` si le PIT ne répond pas (QEMU TCG) ou mesure invalide.
///
/// RÈGLE PIT-QEMU-01 : fonction peut bloquer sur QEMU TCG — appelant doit
/// s'assurer d'un fallback dans la chaîne de calibration.
pub fn calibrate_tsc_via_pit() -> Option<u64> {
    PIT_CALIBRATION_COUNT.fetch_add(1, Ordering::Relaxed);

    let result = run_pit_calibration(PIT_COUNT_10MS);
    PIT_LAST_QUALITY.store(result.quality as u32, Ordering::Relaxed);

    if result.qemu_tcg_suspect {
        PIT_QEMU_TCG_DETECTED.store(true, Ordering::Relaxed);
    }

    if result.valid { Some(result.tsc_hz) } else { None }
}

/// Exécute la calibration PIT interne et retourne le résultat détaillé.
pub fn run_pit_calibration(count: u16) -> PitCalibrationResult {
    // Setup canal 2.
    setup_ch2_oneshot(count);

    // Lire TSC de départ (sérialisé via LFENCE).
    let tsc_start = tsc_serialized_start();

    // Attendre fin du one-shot.
    let done = wait_ch2_done();
    disable_ch2();

    if !done {
        // QEMU TCG suspect (timeout).
        return PitCalibrationResult {
            tsc_hz: 0,
            tsc_delta: 0,
            valid: false,
            quality: 0,
            qemu_tcg_suspect: true,
        };
    }

    // Lire TSC de fin (RDTSCP + LFENCE).
    let tsc_end = tsc_serialized_end();
    let tsc_delta = tsc_end.wrapping_sub(tsc_start);

    if tsc_delta == 0 {
        return PitCalibrationResult {
            tsc_hz: 0, tsc_delta: 0, valid: false, quality: 0, qemu_tcg_suspect: true,
        };
    }

    // Calcul de la fréquence : tsc_hz = tsc_delta × PIT_FREQ_HZ / count
    let tsc_hz_raw = (tsc_delta as u128 * PIT_FREQ_HZ as u128 / count as u128) as u64;

    // Validation plage physique.
    let valid = tsc_hz_raw >= 100_000_000 && tsc_hz_raw <= 10_000_000_000;

    // Indicateur de qualité : dépend de la valeur de tsc_delta.
    // Un delta trop petit (< 100_000 cycles) → mesure peu précise.
    let quality = if !valid {
        0
    } else if tsc_delta < 100_000 {
        30
    } else if tsc_delta < 1_000_000 {
        60
    } else {
        85 // PIT maximum quality (inférieur à HPET car moins précis)
    };

    // Détection QEMU TCG : si tsc_hz < 200 MHz → très probablement TCG.
    let qemu_tcg_suspect = valid && tsc_hz_raw < 200_000_000;

    PitCalibrationResult {
        tsc_hz: tsc_hz_raw,
        tsc_delta,
        valid,
        quality,
        qemu_tcg_suspect,
    }
}

// ── Détection QEMU ────────────────────────────────────────────────────────────

/// Retourne `true` si QEMU TCG a été détecté lors des calibrations PIT.
pub fn qemu_tcg_detected() -> bool {
    PIT_QEMU_TCG_DETECTED.load(Ordering::Relaxed)
}

/// Tente de détecter QEMU TCG via KVM CPUID leaf 0x40000001.
/// KVM réel : bit 0 de EAX = "KVM_FEATURE_CLOCKSOURCE". TCG : CPUID retourne 0.
pub fn detect_kvm_vs_tcg() -> bool {
    // Vérifier hyperviseur présent.
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x0000_0001u32 => _,
            inout("ecx") 0u32 => ecx,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    if (ecx & (1 << 31)) == 0 { return false; } // Pas d'hyperviseur.

    // KVM leaf 0x40000001 → EAX = KVM features.
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 0x4000_0001u32 => eax,
            inout("ecx") 0u32 => _,
            out("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem)
        );
    }
    // Bit 0 = KVM_FEATURE_CLOCKSOURCE (KVM réel).
    (eax & 1) != 0
}

/// Retourne l'indicateur de qualité de la dernière calibration PIT (0-100).
pub fn last_calibration_quality() -> u8 {
    PIT_LAST_QUALITY.load(Ordering::Relaxed) as u8
}

// ── Primitives I/O ────────────────────────────────────────────────────────────

/// Écriture I/O octet.
#[inline(always)]
unsafe fn outb_raw(port: u16, val: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nostack, nomem)
        );
    }
}

/// Lecture I/O octet.
#[inline(always)]
unsafe fn inb_raw(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") val,
            options(nostack, nomem)
        );
    }
    val
}

/// Délai I/O (port 0x80 write, ≈1µs sur ISA bus).
#[inline(always)]
unsafe fn io_delay() {
    unsafe { core::arch::asm!("out 0x80, al", in("al") 0u8, options(nostack, nomem)); }
}

/// Lecture TSC sérialisée (LFENCE avant RDTSC).
#[inline(always)]
fn tsc_serialized_start() -> u64 {
    let lo: u32; let hi: u32;
    unsafe {
        core::arch::asm!(
            "lfence",
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Lecture TSC sérialisée (RDTSCP + LFENCE).
#[inline(always)]
fn tsc_serialized_end() -> u64 {
    let lo: u32; let hi: u32; let _aux: u32;
    unsafe {
        core::arch::asm!(
            "rdtscp",
            "lfence",
            out("eax") lo,
            out("edx") hi,
            out("ecx") _aux,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | lo as u64
}
