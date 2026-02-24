// kernel/src/scheduler/timer/clock.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Horloge kernel — RDTSC calibré, temps monotone et réel
// ═══════════════════════════════════════════════════════════════════════════════
//
// Utilise RDTSC comme source de temps rapide.
// Le calibrage (tsc_hz) est fait une fois au boot par arch_calibrate_tsc().
// Formule : ns = tsc_delta × NS_PER_SEC / tsc_hz
//         = tsc_delta × 1_000_000_000 / tsc_hz
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Fréquence TSC (Hz) calibrée au boot
// ─────────────────────────────────────────────────────────────────────────────

static TSC_HZ: AtomicU64 = AtomicU64::new(3_000_000_000); // Valeur par défaut 3GHz.
static TSC_AT_BOOT: AtomicU64 = AtomicU64::new(0);

/// Initialise l'horloge avec la fréquence TSC mesurée.
///
/// # Safety
/// Doit être appelé une seule fois, par le BSP, avant tout accès au temps.
pub unsafe fn init(tsc_hz: u64) {
    TSC_HZ.store(tsc_hz, Ordering::Release);
    TSC_AT_BOOT.store(rdtsc(), Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Lecture TSC brut
// ─────────────────────────────────────────────────────────────────────────────

/// Lit le TSC via RDTSC. Non sérialisé — usage perf uniquement.
#[inline(always)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)); }
    ((hi as u64) << 32) | lo as u64
}

/// Lit le TSC via RDTSCP (sérialisé côté lecture : barrière store-load).
#[inline(always)]
pub fn rdtscp() -> u64 {
    let lo: u32;
    let hi: u32;
    let _aux: u32;
    unsafe { core::arch::asm!("rdtscp", out("eax") lo, out("edx") hi, out("ecx") _aux, options(nomem, nostack)); }
    ((hi as u64) << 32) | lo as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// Conversion TSC → nanosecondes
// ─────────────────────────────────────────────────────────────────────────────

#[inline(always)]
pub fn tsc_to_ns(tsc_delta: u64) -> u64 {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 { return 0; }
    // Évite l'overflow : multiplication 128-bit simulée.
    // Pour des valeurs typiques (delta < 2^40, hz ~ 3e9), pas de dépassement de u128.
    ((tsc_delta as u128 * 1_000_000_000u128) / hz as u128) as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// Temps monotone (ns depuis le boot)
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le temps en nanosecondes depuis le boot (horloge monotone).
///
/// Source : RDTSC − TSC_AT_BOOT, converti en ns.
/// Précision : ≈ 1 cycle TSC (< 1 ns à 1+ GHz).
#[inline]
pub fn monotonic_ns() -> u64 {
    let t = rdtsc();
    let t0 = TSC_AT_BOOT.load(Ordering::Relaxed);
    tsc_to_ns(t.wrapping_sub(t0))
}

/// Retourne le temps en microsecondes depuis le boot.
#[inline]
pub fn monotonic_us() -> u64 {
    monotonic_ns() / 1_000
}

// ─────────────────────────────────────────────────────────────────────────────
// Temps réel (ns depuis l'epoch UNIX)
// ─────────────────────────────────────────────────────────────────────────────

/// Décalage entre le temps monotone et l'epoch UNIX (ns).
static REALTIME_OFFSET_NS: AtomicU64 = AtomicU64::new(0);

/// Synchronise l'horloge réelle (appelé depuis RTC ou NTP userspace).
///
/// # Safety
/// Thread-safe via AtomicU64.
pub fn set_realtime_offset(epoch_ns: u64) {
    let mono = monotonic_ns();
    REALTIME_OFFSET_NS.store(epoch_ns.wrapping_sub(mono), Ordering::Release);
}

/// Retourne le temps réel Unix en nanosecondes.
pub fn realtime_ns() -> u64 {
    let offset = REALTIME_OFFSET_NS.load(Ordering::Relaxed);
    monotonic_ns().wrapping_add(offset)
}
