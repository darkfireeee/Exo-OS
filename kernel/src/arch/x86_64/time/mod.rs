// kernel/src/arch/x86_64/time/mod.rs
//
// ════════════════════════════════════════════════════════════════════════════
// Module Time — Timekeeping unifié pour Exo-OS
// ════════════════════════════════════════════════════════════════════════════
//
// ## Architecture
//
//   time/
//   ├── ktime.rs          — Horloge monotone seqlock (ktime_get_ns) — primitif universel
//   ├── sources/          — Abstraction sources d'horloge (HPET, PM Timer, TSC, PIT)
//   ├── calibration/      — Calibration TSC multi-source avec fenêtre temporelle réelle
//   ├── drift/            — Correction dérive TSC (software PLL ±500 ppm)
//   └── percpu/           — Offsets TSC per-CPU pour SMP
//
// ## Règles critiques
//   ARCH-TIME-01 : scheduler/timer/clock.rs DOIT déléguer à ktime_get_ns()
//   ARCH-TIME-02 : Seule source de temps autorisée depuis le kernel
//   ARCH-TIME-03 : Seqlock obligatoire pour les horloges globales
//   ARCH-TIME-04 : ktime ne régresse JAMAIS
//
// ## Point d'entrée
//   `time_init()` remplace la Phase 2c de kernel_init() :
//     - init_hpet_post_memory()     → HPET remappage UC + activation compteur
//     - calibrate_tsc()             → fenêtre multi-source + fallback chain
//     - pll_init(hz)                → software PLL initialisé
//     - init_ktime(tsc_now, 0, hz)  → seqlock activé
//     - clock::init(hz)             → scheduler clock compatibility shim
//     - init_bsp_percpu()           → offset BSP = 0
// ════════════════════════════════════════════════════════════════════════════


// ── Sous-modules ──────────────────────────────────────────────────────────────

pub mod ktime;
pub mod sources;
pub mod calibration;
pub mod drift;
pub mod percpu;

// ── Ré-exports fondamentaux ───────────────────────────────────────────────────

// Primitif universel d'horloge — le seul chemin d'accès au temps depuis le kernel.
pub use ktime::{
    ktime_get_ns,
    ktime_get_wall_ns,
    set_wall_time,
    ktime_rtoffset_ns,
    ktime_elapsed_ns,
    ktime_elapsed_us,
    ktime_past,
    ktime_not_past,
    ktime_deadline_ns,
    ktime_deadline_us,
    ktime_deadline_ms,
    ktime_tsc_hz,
    ktime_initialized,
    ktime_contention_count,
    ktime_snapshot,
    KtimeSnapshot,
    tsc_offset,
};

// ── Initialisation ────────────────────────────────────────════════════════────

/// Initialise tout le système de timekeeping.
///
/// Remplace la Phase 2c de `kernel_init()`.
///
/// ## Séquence d'initialisation
/// 1. **HPET post-memory** : remappage UC + activation du compteur HPET.
///    Doit être fait après `hybrid::init()` (buddy allocator opérationnel pour map_4k_page).
/// 2. **Calibration TSC** : fenêtre multi-source avec chaîne de fallback complète.
///    Écrit sur port 0xE9 la source utilisée pour faciliter le diagnostic.
/// 3. **Software PLL** : initialisation avec la fréquence calibrée.
/// 4. **Per-CPU BSP** : offset TSC du BSP (CPU 0) = 0 (référence absolue).
/// 5. **KtimeState seqlock** : active l'horloge monotone.
/// 6. **Scheduler clock shim** : compatibilité avec les consommateurs existants.
///
/// # Safety
/// - Doit être appelé UNE SEULE FOIS depuis le BSP.
/// - Doit être appelé APRÈS `hybrid::init()` (besoin de l'allocateur buddy).
/// - Doit être appelé AVANT `scheduler::init()` (tick handler appelle ktime).
#[allow(unused_must_use)]
pub unsafe fn time_init() {
    // ── Étape 1 : HPET remappage UC + activation ──────────────────────────────
    let _ = crate::arch::x86_64::acpi::hpet::init_hpet_post_memory();

    // ── Étape 2 : Calibration TSC via fenêtre temporelle réelle ───────────────
    let tsc_hz = calibration::calibrate_tsc();

    // ── Étape 3 : Software PLL initialisé avec fréquence calibrée ─────────────
    drift::drift_init(tsc_hz);

    // ── Étape 4 : Offset BSP = 0 (référence SMP) ──────────────────────────────
    percpu::init_bsp_percpu();

    // ── Étape 5 : Lire TSC d'ancrage ─────────────────────────────────────────
    let tsc_now = rdtscp_init();

    // ── Étape 6 : Initialiser le seqlock ktime ────────────────────────────────
    ktime::init_ktime(tsc_now, 0, tsc_hz);

    // ── Étape 7 : Scheduler clock compatibility shim ─────────────────────────
    crate::scheduler::timer::clock::init(tsc_hz);

    // ── Log diagnostic ────────────────────────────────────────────────────────
    debug_log_time_init(tsc_hz);
}

// ── Primitives locales ────────────────────────────────────────────────────────

/// Lit le TSC avec sérialisation complète (RDTSCP + LFENCE).
/// Utilisé uniquement pour l'ancrage initial dans `time_init()`.
#[inline(always)]
unsafe fn rdtscp_init() -> u64 {
    let lo: u32; let hi: u32;
    // FIX TCG: utilise RDTSC simple (sans RDTSCP ni LFENCE) pour compatibilité QEMU TCG.
    // RDTSCP peut bloquer sur QEMU TCG si ECX (TSC_AUX) n'est pas initialisé.
    // LFENCE peut aussi être très lente dans certains modes TCG.
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | lo as u64
}

/// Émet "[TIME-INIT hz=XXXXXXXXXXXXXXXX]\n" sur port 0xE9 pour diagnostic QEMU.
unsafe fn debug_log_time_init(tsc_hz: u64) {
    #[inline(always)]
    unsafe fn out(b: u8) {
        core::arch::asm!("out 0xe9, al", in("al") b, options(nomem, nostack));
    }
    for &b in b"[TIME-INIT hz=" { out(b); }
    // Écrire tsc_hz en décimal (max 11 digits pour 10 GHz).
    let mut buf = [0u8; 20];
    let mut n = tsc_hz;
    let mut i = 19usize;
    if n == 0 { buf[i] = b'0'; } else {
        while n > 0 { buf[i] = b'0' + (n % 10) as u8; n /= 10; i = i.saturating_sub(1); }
        i = i.wrapping_add(1);
    }
    while i <= 19 { out(buf[i]); i += 1; }
    out(b']');
    out(b'\n');
}
