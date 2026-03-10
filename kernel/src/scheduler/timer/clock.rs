// kernel/src/scheduler/timer/clock.rs
//
// ═════════════════════════════════════════════════════════════════════════════
// Horloge scheduler — shim vers ktime_get_ns() (Exo-OS · Couche 1)
// ═════════════════════════════════════════════════════════════════════════════
//
// ## Règle ARCH-TIME-01
//   scheduler/timer/clock.rs NE mesure PAS le TSC, NE calibre PAS.
//   scheduler_now_ns() = ktime_get_ns() — une ligne.
//   Le TSC et sa calibration appartiennent EXCLUSIVEMENT à
//   `arch/x86_64/time/` (sources/, calibration/, ktime.rs).
//
// ## Ce que ce module fournit
//   - `monotonic_ns()` / `monotonic_us()` — temps monotone, délèguent à ktime
//   - `realtime_ns()` — temps UNIX, délègue à ktime
//   - `scheduler_now_ns()` — alias sémantique pour le tick handler
//   - `rdtsc()` / `rdtscp()` — lectures brutes pour profiling perf uniquement
//     (pas pour le temps kernel — RÈGLE ARCH-TIME-02)
//   - `init()` — shim de compatibilité ; en Phase 2b ktime est init'd par
//     `time_init()` avant le scheduler, donc init() ici ne fait rien.
//
// ## Ce que ce module NE fournit PAS (supprimé par rapport à Phase 2a)
//   - TSC_HZ local          → utiliser `ktime::ktime_tsc_hz()` directement
//   - TSC_AT_BOOT local     → utiliser `ktime::ktime_get_ns()` au besoin
//   - tsc_to_ns() local     → utiliser `ktime::ktime_get_ns()` / déltas ktime
//   Ces doublons violaient ARCH-TIME-01 et créaient une incohérence si la
//   dérive TSC était corrigée dans ktime mais pas dans ce module.
// ═════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Shim d'initialisation de compatibilité — no-op en Phase 2b.
///
/// En Phase 2b, `time::time_init()` est appelé avant `scheduler::init()`.
/// `ktime_get_ns()` est donc déjà opérationnel quand le scheduler démarre.
///
/// Conservé pour ne pas casser les call-sites existants.
///
/// # Safety
/// Appelable depuis le BSP uniquement, avant l'activation du SMP.
#[allow(unused_variables)]
pub unsafe fn init(_tsc_hz: u64) {
    // Phase 2b : rien à faire ici.
    // La fréquence TSC est stockée dans KtimeState (ktime.rs)
    // et mise à jour par drift/periodic.rs.
    //
    // Si ktime n'est pas encore initialisé (erreur de séquence), on détecte
    // cela via l'assertion dans hrtimer::init() :
    //   assert!(ktime_get_ns() > 0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Lectures TSC brutes — perf et diagnostics UNIQUEMENT
// ─────────────────────────────────────────────────────────────────────────────
//
// RÈGLE ARCH-TIME-02 : Ces fonctions NE DOIVENT PAS être utilisées pour
// mesurer le temps kernel (timeouts, deltas, deadlines).
// Utiliser ktime_get_ns() / monotonic_ns() à la place.
// Ces fonctions restent utiles pour le profiling micro (mesure de latence
// d'une fonction en cycles) là où ktime serait trop lourd.

/// Lit le TSC via RDTSC (non sérialisé) — usage profiling uniquement.
///
/// RÈGLE ARCH-TIME-02 : NE PAS utiliser pour les délais/timeouts noyau.
/// Pour le temps kernel, appeler `monotonic_ns()`.
#[inline(always)]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: RDTSC est disponible sur tout x86_64.
    //         Non sérialisé — peut être réordonné par le CPU ou le compilateur.
    //         Acceptable uniquement pour le profiling, pas pour le temps kernel.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Lit le TSC via RDTSCP (sérialisé côté lecture) — usage profiling uniquement.
///
/// RÈGLE ARCH-TIME-02 : NE PAS utiliser pour les délais/timeouts noyau.
/// Préférer `monotonic_ns()` qui intègre l'offset per-CPU SMP.
#[inline(always)]
pub fn rdtscp() -> u64 {
    let lo: u32;
    let hi: u32;
    let _aux: u32;
    // SAFETY: RDTSCP disponible sur x86_64 moderne (Nehalem+).
    //         Sérialisé côté lecture (barrière store-load implicite).
    unsafe {
        core::arch::asm!(
            "rdtscp",
            out("eax") lo,
            out("edx") hi,
            out("ecx") _aux,
            options(nomem, nostack)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// Temps monotone (ns depuis le boot) — RÈGLE ARCH-TIME-01
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le temps monotone en nanosecondes depuis le boot.
///
/// RÈGLE ARCH-TIME-01 : délègue à `ktime_get_ns()` — une ligne.
/// Utilise le seqlock ISR-safe + TSC calibré + offset per-CPU SMP.
///
/// Alias sémantique pour le tick handler et le scheduler :
/// `scheduler_now_ns()` → `monotonic_ns()` → `ktime_get_ns()`.
#[inline]
pub fn monotonic_ns() -> u64 {
    crate::arch::x86_64::time::ktime::ktime_get_ns()
}

/// Temps monotone en microsecondes depuis le boot.
#[inline]
pub fn monotonic_us() -> u64 {
    monotonic_ns() / 1_000
}

/// Alias sémantique utilisé par le tick handler et pick_next_task().
///
/// RÈGLE ARCH-TIME-01 : scheduler_now_ns() = ktime_get_ns() — une ligne.
/// Ne lit pas le TSC directement — applique l'offset per-CPU SMP.
#[inline(always)]
pub fn scheduler_now_ns() -> u64 {
    crate::arch::x86_64::time::ktime::ktime_get_ns()
}

/// Retourne le delta en ns entre maintenant et `start_ns`.
///
/// RÈGLE ARCH-TIME-04 : comme ktime ne régresse jamais, le résultat est ≥ 0.
#[inline(always)]
pub fn elapsed_since_ns(start_ns: u64) -> u64 {
    monotonic_ns().wrapping_sub(start_ns)
}

// ─────────────────────────────────────────────────────────────────────────────
// Temps réel (ns depuis l'epoch UNIX)
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le temps réel UNIX en nanosecondes.
///
/// RÈGLE ARCH-TIME-01 : délègue à `ktime_get_wall_ns()`.
/// La synchronisation wall-clock est gérée par ktime.rs via `set_wall_time()`.
#[inline]
pub fn realtime_ns() -> u64 {
    crate::arch::x86_64::time::ktime::ktime_get_wall_ns()
}

/// Met à jour l'horloge réelle (appelé depuis RTC/NTP userspace).
///
/// Délègue à `ktime::set_wall_time()` pour garantir la cohérence seqlock.
///
/// # Safety
/// Appelable une seule fois par source de synchronisation (RTC ou NTP).
/// Thread-safe via le seqlock WALL_STATE dans ktime.rs.
pub unsafe fn set_realtime_from_epoch(epoch_ns: u64) {
    crate::arch::x86_64::time::ktime::set_wall_time(epoch_ns);
}

/// Retourne l'offset UNIX en ns (wall_ns = monotonic_ns + offset).
#[inline]
pub fn realtime_offset_ns() -> u64 {
    crate::arch::x86_64::time::ktime::ktime_rtoffset_ns()
}
