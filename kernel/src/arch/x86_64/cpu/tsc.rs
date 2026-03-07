//! # cpu/tsc.rs — TSC (Time Stamp Counter) calibration et wrappers
//!
//! Fournit un accès fiable au TSC pour la mesure de temps noyau.
//!
//! ## Politique
//! - Sur CPUs modernes : invariant TSC garanti → utilisé comme clock monotone
//! - Calibration via HPET ou PIT comme référence
//! - TSC_AUX configuré avec le CPU ID logique pour RDTSCP
//!
//! ## Précision cible
//! Calibration à ±0.01% (< 100 ppm)

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ── TSC globals ───────────────────────────────────────────────────────────────

/// Fréquence du TSC en Hz (déterminée à la calibration)
static TSC_HZ: AtomicU64 = AtomicU64::new(0);

/// Fréquence du TSC en kHz (arrondie)
static TSC_KHZ: AtomicU64 = AtomicU64::new(0);

/// TSC invariant disponible (invariant_tsc = constant rate across C-states)
static TSC_INVARIANT: AtomicBool = AtomicBool::new(false);

/// Valeur TSC au boot (point de référence absolu)
static TSC_BOOT_VALUE: AtomicU64 = AtomicU64::new(0);

/// Calibration terminée
static TSC_CALIBRATED: AtomicBool = AtomicBool::new(false);

// ── Lecture TSC ───────────────────────────────────────────────────────────────

/// Lit le TSC avec RDTSC (non-sérialisante)
///
/// Pour des mesures précises avec barrière, utiliser `read_tsc_serialized()`.
#[inline(always)]
pub fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC est non-privilégiée sur x86_64 — aucun effet de bord
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Lit le TSC avec barrière LFENCE (sérialisatrice des loads)
///
/// Garantit que toutes les instructions précédentes ont complété avant la lecture.
/// Utilisé pour débuter une mesure précise.
#[inline(always)]
pub fn read_tsc_begin() -> u64 {
    // SAFETY: LFENCE + RDTSC : séquence standard pour mesure précise
    unsafe {
        core::arch::asm!("lfence", options(nostack, nomem, preserves_flags));
    }
    read_tsc()
}

/// Lit le TSC avec barrière RDTSCP + LFENCE (sérialisatrice des deux côtés)
///
/// Garantit que toutes les instructions précédentes ET celle-ci ont complété.
/// Utilisé pour terminer une mesure précise.
///
/// Retourne `(tsc_value, cpu_aux)` — cpu_aux = CPU ID logique si configuré.
#[inline(always)]
pub fn read_tsc_end() -> (u64, u32) {
    let lo: u32; let hi: u32; let aux: u32;
    // SAFETY: RDTSCP est sérialisante pour les loads précédents
    unsafe {
        core::arch::asm!(
            "rdtscp",
            out("eax") lo,
            out("edx") hi,
            out("ecx") aux,
            options(nostack, nomem)
        );
        // Barrière post pour empêcher les instructions suivantes de remonter
        core::arch::asm!("lfence", options(nostack, nomem, preserves_flags));
    }
    (((hi as u64) << 32) | (lo as u64), aux)
}

/// Délai TSC en nanosecondes
///
/// Attend `ns` nanosecondes en utilisant le TSC comme référence.
/// Nécessite que `TSC_HZ` soit calibré.
pub fn tsc_delay_ns(ns: u64) {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        // TSC non calibré — fallback boucle simple
        for _ in 0..ns * 10 {
            core::hint::spin_loop();
        }
        return;
    }
    // cycles = ns * hz / 1_000_000_000
    // Utilise multiplication 128 bits pour éviter overflow
    let cycles = (ns as u128 * hz as u128) / 1_000_000_000;
    let start = read_tsc();
    while (read_tsc().wrapping_sub(start)) < cycles as u64 {
        core::hint::spin_loop();
    }
}

/// Délai TSC en microsecondes
#[inline]
pub fn tsc_delay_us(us: u64) {
    tsc_delay_ns(us * 1_000);
}

/// Délai TSC en millisecondes
#[inline]
pub fn tsc_delay_ms(ms: u64) {
    tsc_delay_ns(ms * 1_000_000);
}

// ── Conversion TSC → temps ────────────────────────────────────────────────────

/// Convertit des cycles TSC en nanosecondes
pub fn tsc_cycles_to_ns(cycles: u64) -> u64 {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 { return cycles; }
    // ns = cycles * 1_000_000_000 / hz
    (cycles as u128 * 1_000_000_000 / hz as u128) as u64
}

/// Retourne le temps écoulé depuis le boot en nanosecondes (monotone)
pub fn tsc_ns_since_boot() -> u64 {
    let boot = TSC_BOOT_VALUE.load(Ordering::Relaxed);
    let now  = read_tsc();
    tsc_cycles_to_ns(now.wrapping_sub(boot))
}

// ── Calibration via PIT ───────────────────────────────────────────────────────

/// Durée de calibration PIT en ticks (PIT tick ≈ 838 ns → 10ms = ~11932 ticks)
const PIT_CALIBRATE_COUNT: u16 = 11931;

/// Port PIT canal 2 (speaker — peut être utilisé sans déclencher de son)
const PIT_CHANNEL2: u16 = 0x42;
const PIT_CONTROL:  u16 = 0x43;
const PIT_GATE:     u16 = 0x61;

/// PIT fréquence base en Hz
const PIT_BASE_HZ: u64 = 1_193_182;

/// Calibre le TSC en utilisant le PIT canal 2 comme référence
///
/// Durée : environ 10ms (bloquant)
/// Précision : ±1% (suffisant pour init, HPET affinera si disponible)
pub fn calibrate_tsc_with_pit() -> u64 {
    use super::super::{outb, inb, io_delay};

    // 1. Configurer PIT canal 2 en mode 0 (one-shot)
    // SAFETY: ports PIT valides — utilisation standard
    unsafe {
        outb(PIT_CONTROL, 0xB0); // Canal 2, mode 0, binaire, LSB+MSB
        io_delay();
        outb(PIT_CHANNEL2, (PIT_CALIBRATE_COUNT & 0xFF) as u8);
        io_delay();
        outb(PIT_CHANNEL2, (PIT_CALIBRATE_COUNT >> 8) as u8);
        io_delay();

        // 2. Activer gate PIT canal 2 (bit 0 du port 0x61) + désactiver speaker (bit 1=0)
        let gate = inb(PIT_GATE);
        outb(PIT_GATE, (gate | 0x01) & !0x02);
        io_delay();
    }

    // 3. Lire TSC de départ (avec barrière)
    let tsc_start = read_tsc_begin();

    // 4. Attendre que le PIT expire (OUTPUT bit 5 du port 0x61 = 0 → 1)
    // Timeout de secours via compteur d'itérations (n'utilise pas le TSC pour ne
    // pas dépendre de sa fréquence inconnue à ce stade).
    // À ~1 ns/iter (séquence inb + test + jmp ≈ 10 cycles à 10 GHz), 10M iter ≈ 10ms.
    // À 100 pico-s/iter, 10M iters = 1ms. Au total, 200M iters offre un safety margin.
    // En pratique : si le PIT fonctionne, on sort après ~10ms réels.
    //              Sinon on sort après au plus ~2ms @ 10 GHz / ~200ms @ 1 GHz emulé.
    let _ = tsc_start; // sera utilisé en dessous si pit_ok
    let mut pit_ok = false;
    let mut counter: u32 = 0;
    const MAX_ITER: u32 = 200_000_000; // ~100-200ms max, conservative
    loop {
        let val = unsafe { inb(PIT_GATE) };
        if val & 0x20 != 0 { pit_ok = true; break; }
        counter = counter.wrapping_add(1);
        if counter == 0 || counter >= MAX_ITER { break; } // timeout
        core::hint::spin_loop();
    }
    if !pit_ok {
        // PIT canal 2 non fonctionnel — signaler l'échec au caller
        return 0;
    }

    // 5. Lire TSC de fin
    let (tsc_end, _) = read_tsc_end();

    // 6. Désactiver gate PIT canal 2
    // SAFETY: restauration de l'état du port 0x61
    unsafe {
        let gate = inb(PIT_GATE);
        outb(PIT_GATE, gate & !0x01);
    }

    // 7. Calculer fréquence
    let tsc_delta = tsc_end.wrapping_sub(tsc_start);
    // 10ms = PIT_CALIBRATE_COUNT / PIT_BASE_HZ secondes
    // hz = tsc_delta * PIT_BASE_HZ / PIT_CALIBRATE_COUNT / 0.01
    //    = tsc_delta * PIT_BASE_HZ / PIT_CALIBRATE_COUNT * 100
    let hz = (tsc_delta as u128 * PIT_BASE_HZ as u128 / PIT_CALIBRATE_COUNT as u128 * 100) as u64;
    hz
}

/// Calibration TSC via CPUID (Intel uniquement — leaf 0x15)
///
/// Sur les CPUs modernes Intel, CPUID 0x15 donne le ratio TSC/bus clock exact.
/// Retourne `Some(hz)` si supporté, `None` sinon.
pub fn calibrate_tsc_cpuid() -> Option<u64> {
    let (eax, ebx, ecx, _) = {
        let (eax, ecx, edx): (u32, u32, u32);
        let ebx_r: u64;
        // SAFETY: CPUID non-privilégié ; xchg pour préserver rbx
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") 0x15u32 => eax,
                inout("ecx") 0u32    => ecx,
                out("edx") edx,
                tmp = inout(reg) 0u64 => ebx_r,
                options(nostack, nomem)
            );
        }
        (eax, ebx_r as u32, ecx, edx)
    };

    if eax == 0 || ebx == 0 { return None; }

    // Fréquence TSC = ecx * ebx / eax (Hz)
    // ecx = crystal clock Hz (ou 0 → utiliser valeur standard)
    let crystal_hz: u64 = if ecx != 0 {
        ecx as u64
    } else {
        // Intel recommande 24 MHz pour les CPUs récents sans crystal Hz dans CPUID
        24_000_000
    };

    Some(crystal_hz * ebx as u64 / eax as u64)
}

/// Initialise le TSC : calibration, vérification, configuration TSC_AUX
///
/// Appelé depuis `early_init.rs` après init CPU features et PIC/APIC.
pub fn init_tsc(cpu_logical_id: u32) {
    use super::msr;
    use super::features::CPU_FEATURES;

    // Enregistrer la valeur TSC au boot
    let boot_tsc = read_tsc();
    TSC_BOOT_VALUE.store(boot_tsc, Ordering::Release);

    // Vérifier TSC invariant (CPUID 0x80000007 EDX bit 8)
    let (_,_,_,edx_ext7) = {
        let (ecx, edx): (u32, u32);
        // SAFETY: CPUID non-privilégié ; xchg pour préserver rbx
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") 0x8000_0007u32 => _,
                inout("ecx") 0u32           => ecx,
                out("edx") edx,
                tmp = inout(reg) 0u64 => _,
                options(nostack, nomem)
            );
        }
        (0u32, 0u32, ecx, edx)
    };
    let invariant = (edx_ext7 & (1 << 8)) != 0;
    TSC_INVARIANT.store(invariant, Ordering::Release);

    // Calibrer la fréquence TSC : CPUID 0x15 en priorité, puis 1GHz fallback.
    // NOTE: la calibration via PIT canal 2 (calibrate_tsc_with_pit) est désactivée
    // au démarrage car elle bloque si les callbacks timer QEMU/VM ne s'exécutent
    // pas pendant la boucle d'attente busy-poll (comportement fréquent en TCG/KVM
    // avant init APIC). Elle pourra être réactivée post-boot une fois le timer APIC
    // ou HPET initialisé, via recalibrate_tsc_with_hpet().
    let hz = calibrate_tsc_cpuid()
        .unwrap_or(1_000_000_000); // 1 GHz : fallback sûr ; sera recalibré post-APIC

    // Arrondir à multiple de 100 kHz pour stabilité
    let hz_rounded = (hz + 50_000) / 100_000 * 100_000;

    TSC_HZ.store(hz_rounded, Ordering::Release);
    TSC_KHZ.store(hz_rounded / 1000, Ordering::Release);
    TSC_CALIBRATED.store(true, Ordering::Release);

    // Configurer TSC_AUX avec le CPU ID logique (utilisé par RDTSCP)
    if CPU_FEATURES.has_rdtscp() {
        // SAFETY: MSR_TSC_AUX toujours disponible si RDTSCP supporté
        unsafe { msr::write_msr(msr::MSR_TSC_AUX, cpu_logical_id as u64); }
    }

    // Activer NXE dans EFER si NX disponible
    if CPU_FEATURES.has_nx() {
        // SAFETY: activation NXE dans EFER — requis pour protections mémoire
        unsafe { msr::set_msr_bits(msr::MSR_IA32_EFER, msr::EFER_NXE); }
    }
}

// ── Accesseurs ─────────────────────────────────────────────────────────────────

/// Retourne la fréquence TSC en Hz
#[inline(always)] pub fn tsc_hz()  -> u64 { TSC_HZ.load(Ordering::Relaxed)  }

/// Retourne la fréquence TSC en kHz
#[inline(always)] pub fn tsc_khz() -> u64 { TSC_KHZ.load(Ordering::Relaxed) }

/// Convertit des millisecondes en cycles TSC
///
/// Retourne 0 si le TSC n'est pas encore calibré.
#[inline(always)]
pub fn tsc_ms_to_cycles(ms: u64) -> u64 {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 { return 0; }
    // ms * hz / 1_000 — évite l'overflow u64 pour ms < 4_000_000 à 4 GHz
    ms.saturating_mul(hz) / 1_000
}

/// Convertit des microsecondes en cycles TSC
///
/// Retourne 0 si le TSC n'est pas encore calibré.
#[inline(always)]
pub fn tsc_us_to_cycles(us: u64) -> u64 {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 { return 0; }
    us.saturating_mul(hz) / 1_000_000
}

/// Retourne `true` si le TSC est invariant (stable à travers les C-states)
#[inline(always)] pub fn tsc_invariant() -> bool { TSC_INVARIANT.load(Ordering::Relaxed) }

/// Retourne `true` si la calibration TSC est terminée
#[inline(always)] pub fn tsc_calibrated() -> bool { TSC_CALIBRATED.load(Ordering::Relaxed) }

// ── Instrumentation ───────────────────────────────────────────────────────────

static TSC_OVERFLOW_COUNT: AtomicU64 = AtomicU64::new(0);

/// Compteur d'overflows TSC détectés (surveillance)
pub fn tsc_overflow_count() -> u64 {
    TSC_OVERFLOW_COUNT.load(Ordering::Relaxed)
}
