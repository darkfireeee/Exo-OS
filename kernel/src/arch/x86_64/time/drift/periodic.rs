// kernel/src/arch/x86_64/time/drift/periodic.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Thread de recalibration périodique de la dérive TSC
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Rôle
//   Mesure périodiquement la dérive du TSC en comparant TSC et HPET (ou PM Timer)
//   sur une fenêtre temporelle réelle, puis corrige la fréquence dans KtimeState
//   via la PLL pour éviter les sauts brutaux.
//
// ## Périodicité
//   - Idle (charge CPU < 20%) : toutes les 30 secondes
//   - Normal                  : toutes les 10 secondes
//   - Charge élevée (> 80%)   : toutes les 5 secondes (drift thermique plus important)
//
// ## Règles critiques
//   RÈGLE DRIFT-PREEMPT-01  : preempt_disable() AVANT la mesure HPET+TSC
//                             → évite que le thread soit préempté entre les deux lectures
//   RÈGLE DRIFT-CIRCULAR-01 : N'appelle PAS ktime_get_ns() ici
//                             → lire HPET et TSC directement via fonctions bas niveau
//   RÈGLE DRIFT-PLL-01      : correction ≤ ±500 ppm par cycle (via pll.rs)
//   RÈGLE DRIFT-MONOTONE-01 : ns_anchor_new >= ns_anchor_old (vérification avant write)
//
// ## Architecture
//   Ce module est appelé périodiquement depuis le tick handler (tick.rs).
//   Il ne maintient PAS de thread propre (trop coûteux en Phase 2b).
//   À terme (Phase 4+) : kthread dédié avec HLT/MWAIT en idle.
// ════════════════════════════════════════════════════════════════════════════════

use super::super::calibration::window;
use super::super::ktime;
use super::super::sources::{hpet as hpet_src, pm_timer as pm_src};
use super::pll;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ── Constantes de périodicité ─────────────────────────────────────────────────

/// Intervalle de recalibration en conditions normales (ns).
const RECAL_INTERVAL_NORMAL_NS: u64 = 10_000_000_000; // 10 secondes
/// Intervalle en charge élevée (ns).
const RECAL_INTERVAL_HEAVY_NS: u64 = 5_000_000_000; // 5 secondes
/// Intervalle en idle (ns).
const RECAL_INTERVAL_IDLE_NS: u64 = 30_000_000_000; // 30 secondes

/// Seuil de charge CPU pour "idle" (pourcentage × 10 = permillage).
const IDLE_THRESHOLD_PERMIL: u32 = 200; // 20%
/// Seuil de charge CPU pour "charge élevée".
const HEAVY_THRESHOLD_PERMIL: u32 = 800; // 80%

/// Taille de l'historique de mesures (FIFO circulaire pour détection d'anomalie).
#[allow(dead_code)]
const HISTORY_SIZE: usize = 8;

// ── État du thread de recalibration ──────────────────────────────────────────

struct DriftState {
    /// Timestamp (ktime ns) de la dernière recalibration.
    last_recal_ns: AtomicU64,
    /// Intervalle de recalibration courant (adaptatif).
    interval_ns: AtomicU64,
    /// Nombre total de recalibrations effectuées depuis le boot.
    recal_count: AtomicU64,
    /// Nombre de recalibrations échouées (HPET/PM Timer non disponible ou mesure hors plage).
    fail_count: AtomicU64,
    /// Charge CPU courante en permillage (0–1000), mise à jour par le tick handler.
    cpu_load_permil: AtomicU32,
    /// `true` si une recalibration est en cours (guard contre ré-entrance SMP).
    in_progress: AtomicBool,
    /// Nombre de fois que la monotonie a dû être corrigée (dérive PLL trop agressive).
    monotone_fixes: AtomicU64,
    /// Dernière fréquence TSC mesurée (avant PLL).
    last_measured_hz: AtomicU64,
    /// Dernière fréquence TSC appliquée (après PLL).
    last_applied_hz: AtomicU64,
}

impl DriftState {
    const fn new() -> Self {
        Self {
            last_recal_ns: AtomicU64::new(0),
            interval_ns: AtomicU64::new(RECAL_INTERVAL_NORMAL_NS),
            recal_count: AtomicU64::new(0),
            fail_count: AtomicU64::new(0),
            cpu_load_permil: AtomicU32::new(0),
            in_progress: AtomicBool::new(false),
            monotone_fixes: AtomicU64::new(0),
            last_measured_hz: AtomicU64::new(0),
            last_applied_hz: AtomicU64::new(0),
        }
    }
}

static DRIFT: DriftState = DriftState::new();

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise le module de correction de dérive.
/// Appelé depuis `time_init()` après la calibration initiale.
pub fn drift_init(initial_hz: u64) {
    pll::pll_init(initial_hz);
    DRIFT.last_applied_hz.store(initial_hz, Ordering::Release);
}

// ── Point d'entrée périodique ─────────────────────────────────────────────────

/// Vérifie si une recalibration est due et l'exécute si nécessaire.
///
/// Appelé depuis le tick handler (tick.rs) à chaque tick (HZ=1000, soit 1ms).
/// Ne bloque PAS — retourne immédiatement si l'intervalle n'est pas écoulé.
///
/// RÈGLE DRIFT-CIRCULAR-01 : Cette fonction lit le TSC directement.
///   Elle N'appelle PAS ktime_get_ns() pour éviter la dépendance circulaire.
/// RÈGLE DRIFT-PREEMPT-01 : preempt_disable() avant la fenêtre de mesure.
pub fn drift_tick(tsc_now_direct: u64) {
    // Lire le dernier instant de recalibration.
    let last = DRIFT.last_recal_ns.load(Ordering::Relaxed);
    let interval = DRIFT.interval_ns.load(Ordering::Relaxed);

    // Calculer le temps écoulé depuis la dernière recalibration.
    // RÈGLE DRIFT-CIRCULAR-01 : On utilise le TSC directement + la fréquence PLL courante,
    // pas ktime_get_ns() qui dépend de tsc_hz en cours de mise à jour.
    let current_hz = pll::pll_current_hz();
    if current_hz == 0 {
        return;
    }

    // Lire le snapshot UNE FOIS pour la cohérence (évite deux lectures atomiques disjointes).
    let snap = ktime::ktime_snapshot();
    let elapsed_tsc = tsc_now_direct.wrapping_sub(snap.tsc_base);
    let elapsed_ns = (elapsed_tsc as u128 * 1_000_000_000 / current_hz as u128) as u64;

    // Temps absolu estimé = ns_base + elapsed depuis le dernier ancrage.
    let estimated_ns = snap.ns_base.wrapping_add(elapsed_ns);

    if last > 0 && estimated_ns.wrapping_sub(last) < interval {
        return; // Pas encore temps de recalibrer.
    }

    // Guard contre ré-entrance SMP : un seul CPU recalibre à la fois.
    if DRIFT
        .in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    // Lancer la recalibration.
    let result = do_recalibrate(tsc_now_direct, estimated_ns);

    // Adapter l'intervalle selon la charge CPU.
    adapt_interval();

    DRIFT.in_progress.store(false, Ordering::Release);

    if result {
        DRIFT.last_recal_ns.store(estimated_ns, Ordering::Release);
        DRIFT.recal_count.fetch_add(1, Ordering::Relaxed);
    } else {
        DRIFT.fail_count.fetch_add(1, Ordering::Relaxed);
    }
}

// ── Cœur de la recalibration ──────────────────────────────────────────────────

/// Effectue une mesure de dérive et applique la correction PLL.
///
/// Retourne `true` si une correction a été appliquée, `false` si la mesure a échoué.
///
/// RÈGLE DRIFT-PREEMPT-01 : preempt désactivé pendant la fenêtre de mesure.
/// RÈGLE DRIFT-CIRCULAR-01 : HPET et TSC lus directement.
fn do_recalibrate(tsc_anchor: u64, ns_anchor: u64) -> bool {
    // Tenter la recalibration via HPET (source préférée).
    let measured_hz = if hpet_src::available() {
        let freq = hpet_src::freq_hz();
        // Fenêtre courte (1ms) pour limiter la durée de preempt_disable.
        // Utiliser la version directe de window.rs (CAL-WINDOW-01 + CAL-CLI-01).
        match window::calibrate_tsc_via_hpet(freq) {
            Some(hz) if is_plausible_hz(hz) => hz,
            _ => {
                // Fallback PM Timer.
                match try_pm_timer_measure() {
                    Some(hz) => hz,
                    None => return false,
                }
            }
        }
    } else if pm_src::available() {
        match try_pm_timer_measure() {
            Some(hz) => hz,
            None => return false,
        }
    } else {
        return false; // Aucune source de référence disponible.
    };

    DRIFT.last_measured_hz.store(measured_hz, Ordering::Relaxed);

    // Calculer le temps absolu courant depuis tsc_anchor + offset.
    // RÈGLE DRIFT-CIRCULAR-01 : on utilise tsc_anchor et ns_anchor passés en paramètre,
    // pas ktime_get_ns().
    let current_hz_pll = pll::pll_current_hz();
    let tsc_since_anchor = rdtsc_direct().wrapping_sub(tsc_anchor);
    let ns_since_anchor = if current_hz_pll > 0 {
        (tsc_since_anchor as u128 * 1_000_000_000 / current_hz_pll as u128) as u64
    } else {
        0
    };
    let ns_now = ns_anchor.wrapping_add(ns_since_anchor);

    // Appliquer la correction PLL.
    let (ns_corrected, new_hz) = pll::pll_update(measured_hz, rdtsc_direct(), ns_now);

    // Vérification monotonie stricte.
    // RÈGLE DRIFT-MONOTONE-01 : ns_corrected >= ktime::ns_base courant.
    let snap = ktime::ktime_snapshot();
    let ns_final = if ns_corrected < snap.ns_base {
        DRIFT.monotone_fixes.fetch_add(1, Ordering::Relaxed);
        snap.ns_base + 1 // Avancer d'au moins 1 ns pour maintenir la monotonie.
    } else {
        ns_corrected
    };

    let tsc_final = rdtsc_direct();

    // Écrire le nouvel ancrage dans KtimeState (seqlock).
    ktime::update_ktime_anchor(tsc_final, ns_final, new_hz);

    DRIFT.last_applied_hz.store(new_hz, Ordering::Relaxed);
    true
}

/// Tente une mesure via PM Timer (fallback HPET absent).
fn try_pm_timer_measure() -> Option<u64> {
    match window::calibrate_tsc_via_pm_timer() {
        Some(hz) if is_plausible_hz(hz) => Some(hz),
        _ => None,
    }
}

// ── Adaptation de l'intervalle ────────────────────────────────────────────────

/// Adapte l'intervalle de recalibration selon la charge CPU.
fn adapt_interval() {
    let load = DRIFT.cpu_load_permil.load(Ordering::Relaxed);
    let new_interval = if load < IDLE_THRESHOLD_PERMIL {
        RECAL_INTERVAL_IDLE_NS
    } else if load > HEAVY_THRESHOLD_PERMIL {
        RECAL_INTERVAL_HEAVY_NS
    } else {
        RECAL_INTERVAL_NORMAL_NS
    };
    DRIFT.interval_ns.store(new_interval, Ordering::Relaxed);
}

/// Met à jour la charge CPU courante (appelé depuis le tick handler).
/// `load_permil` : charge en permillage (0–1000, ex: 500 = 50%).
pub fn update_cpu_load(load_permil: u32) {
    DRIFT
        .cpu_load_permil
        .store(load_permil.min(1000), Ordering::Relaxed);
}

// ── Validations internes ──────────────────────────────────────────────────────

/// Vérifie qu'une fréquence mesurée est dans la plage raisonnable [500 MHz, 10 GHz].
#[inline(always)]
fn is_plausible_hz(hz: u64) -> bool {
    hz >= 500_000_000 && hz <= 10_000_000_000
}

// ── Primitive assembleur locale ───────────────────────────────────────────────

/// Lit le TSC directement (RDTSC non-sérialisé, utilisé pour les mesures de dérive).
/// RÈGLE DRIFT-CIRCULAR-01 : Jamais ktime_get_ns() dans ce contexte.
#[inline(always)]
fn rdtsc_direct() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
    }
    ((hi as u64) << 32) | lo as u64
}

// ── Diagnostics ───────────────────────────────────────────────────────────────

/// Nombre total de recalibrations réussies depuis le boot.
pub fn drift_recal_count() -> u64 {
    DRIFT.recal_count.load(Ordering::Relaxed)
}

/// Nombre de tentatives de recalibration échouées.
pub fn drift_fail_count() -> u64 {
    DRIFT.fail_count.load(Ordering::Relaxed)
}

/// Nombre de corrections de monotonie appliquées.
pub fn drift_monotone_fixes() -> u64 {
    DRIFT.monotone_fixes.load(Ordering::Relaxed)
}

/// Dernière fréquence mesurée avant PLL.
pub fn drift_last_measured_hz() -> u64 {
    DRIFT.last_measured_hz.load(Ordering::Relaxed)
}

/// Dernière fréquence appliquée après PLL.
pub fn drift_last_applied_hz() -> u64 {
    DRIFT.last_applied_hz.load(Ordering::Relaxed)
}

/// Snapshot complet de l'état de dérive pour les outils de profiling.
pub struct DriftSnapshot {
    pub recal_count: u64,
    pub fail_count: u64,
    pub monotone_fixes: u64,
    pub measured_hz: u64,
    pub applied_hz: u64,
    pub interval_ns: u64,
    pub pll_locked: bool,
}

pub fn drift_snapshot() -> DriftSnapshot {
    DriftSnapshot {
        recal_count: DRIFT.recal_count.load(Ordering::Relaxed),
        fail_count: DRIFT.fail_count.load(Ordering::Relaxed),
        monotone_fixes: DRIFT.monotone_fixes.load(Ordering::Relaxed),
        measured_hz: DRIFT.last_measured_hz.load(Ordering::Relaxed),
        applied_hz: DRIFT.last_applied_hz.load(Ordering::Relaxed),
        interval_ns: DRIFT.interval_ns.load(Ordering::Relaxed),
        pll_locked: pll::pll_locked(),
    }
}
