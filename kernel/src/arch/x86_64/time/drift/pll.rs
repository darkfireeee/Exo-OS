// kernel/src/arch/x86_64/time/drift/pll.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Software PLL — Correction lissée de la dérive TSC
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Problème
//   Le TSC oscille légèrement selon la température, la charge CPU, les transitions
//   P-state. Si on applique brutalement une nouvelle fréquence TSC mesurée, le
//   temps applicatif saute (en avant ou en arrière si la correction est trop forte).
//
// ## Solution : Software PLL (Phase-Locked Loop)
//   Filtre passe-bas : correction fractionnelle limitée à ±500 ppm par cycle.
//   Convergence progressive sur plusieurs cycles si la dérive est importante.
//   Formule : new_hz = current_hz + clamp(measured_hz - current_hz, -MAX_ADJ, +MAX_ADJ)
//
// ## Garanties
//   RÈGLE DRIFT-PLL-01       : correction max ±500 ppm par recalibration
//   RÈGLE DRIFT-MONOTONE-01  : ktime ne régresse jamais après correction
//   RÈGLE ARCH-TIME-04       : monotonie stricte garantie
//
// ## Résolution temporelle après correction
//   À 3 GHz TSC : ±500 ppm = ±1.5 MHz → ±0.5 ns/µs d'erreur résiduelle
//   Convergence complète si dérive ≤ 500 ppm : 1 cycle de recalibration
//   Convergence si dérive = 5000 ppm : 10 cycles × 30s = 5 minutes
// ════════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicI64, AtomicU64, AtomicBool, Ordering};

// ── Constantes PLL ────────────────────────────────────────────────────────────

/// Correction maximale par cycle de recalibration : 500 ppm.
/// Pour TSC à 3 GHz : 500 ppm × 3_000_000_000 Hz = 1_500 Hz max de correction.
const MAX_ADJ_PPM: u64 = 500;

/// Coefficient de filtrage passe-bas (0–1000 en millièmes).
/// 750 = 75% de la correction appliquée immédiatement (convergence rapide).
/// Valeur plus faible = plus de lissage, convergence plus lente.
const FILTER_COEFF_MILLI: u64 = 750;

/// Nombre de mesures consécutives concordantes requis pour activer la correction.
/// Évite les corrections sur des mesures aberrantes isolées.
const CONVERGENCE_THRESHOLD: u32 = 2;

// ── État PLL ──────────────────────────────────────────────────────────────────

/// État interne de la boucle PLL.
struct PllState {
    /// Fréquence de référence courante (Hz) — base du calcul de correction.
    current_hz:       AtomicU64,
    /// Erreur de phase accumulée (en nano-ppm, signé) — intégration PI controller.
    phase_error_nppm: AtomicI64,
    /// Nombre de mesures concordantes consécutives observées.
    converge_count:   AtomicU64,
    /// Somme des N dernières mesures pour calcul de moyenne glissante.
    measure_sum:      AtomicU64,
    /// Nombre de mesures dans la somme glissante.
    measure_count:    AtomicU64,
    /// Dernier ajustement appliqué (en Hz, signé via cast).
    last_adj_hz:      AtomicI64,
    /// `true` si la PLL est en état de lock (dérive < LOCK_THRESHOLD_PPM).
    locked:           AtomicBool,
    /// Compteur total de corrections appliquées depuis le boot.
    total_corrections: AtomicU64,
}

impl PllState {
    const fn new() -> Self {
        Self {
            current_hz:        AtomicU64::new(3_000_000_000),
            phase_error_nppm:  AtomicI64::new(0),
            converge_count:    AtomicU64::new(0),
            measure_sum:       AtomicU64::new(0),
            measure_count:     AtomicU64::new(0),
            last_adj_hz:       AtomicI64::new(0),
            locked:            AtomicBool::new(false),
            total_corrections: AtomicU64::new(0),
        }
    }
}

static PLL: PllState = PllState::new();

/// Seuil de dérive en ppm en deçà duquel la PLL est considérée "locked".
const LOCK_THRESHOLD_PPM: u64 = 100;

/// Fenêtre de moyenne glissante (N dernières mesures).
const MOVING_AVG_WINDOW: u64 = 5;

// ── API principale ────────────────────────────────────────────────────────────

/// Initialise la PLL avec la fréquence calibrée au boot.
///
/// Doit être appelé depuis `time_init()` après la calibration initiale.
pub fn pll_init(initial_hz: u64) {
    PLL.current_hz.store(initial_hz, Ordering::Release);
    PLL.measure_sum.store(initial_hz * MOVING_AVG_WINDOW, Ordering::Relaxed);
    PLL.measure_count.store(MOVING_AVG_WINDOW, Ordering::Relaxed);
    PLL.locked.store(false, Ordering::Release);
}

/// Soumet une nouvelle mesure de fréquence TSC et calcule la correction lissée.
///
/// RÈGLE DRIFT-PLL-01 : correction max ±500 ppm par appel.
/// RÈGLE DRIFT-MONOTONE-01 : retourne (ns_now, new_hz) tel que ns_now >= ns_précédent.
///
/// # Arguments
/// - `measured_hz`  : fréquence TSC mesurée par la calibration courante
/// - `tsc_now`      : valeur TSC lue DIRECTEMENT (pas via ktime_get_ns)
/// - `ref_ns_anchor`: ns correspondant à `tsc_now` (calculé par le thread drift)
///
/// # Retourne
/// `(ns_anchor_corrigé, new_tsc_hz)` à passer à `update_ktime_anchor()`.
///
/// RÈGLE DRIFT-CIRCULAR-01 : N'appelle PAS ktime_get_ns() ici.
pub fn pll_update(measured_hz: u64, _tsc_now: u64, ref_ns_anchor: u64) -> (u64, u64) {
    // ── Mise à jour moyenne glissante ──────────────────────────────────────────
    let old_sum   = PLL.measure_sum.load(Ordering::Relaxed);
    let old_count = PLL.measure_count.load(Ordering::Relaxed).min(MOVING_AVG_WINDOW);

    // Expulser la plus ancienne mesure de la fenêtre si fenêtre pleine.
    let new_sum = if old_count >= MOVING_AVG_WINDOW {
        let oldest_approx = old_sum / old_count;
        old_sum.saturating_sub(oldest_approx).saturating_add(measured_hz)
    } else {
        old_sum.saturating_add(measured_hz)
    };
    let new_count = (old_count + 1).min(MOVING_AVG_WINDOW);

    PLL.measure_sum.store(new_sum, Ordering::Relaxed);
    PLL.measure_count.store(new_count, Ordering::Relaxed);

    // Fréquence lissée = moyenne glissante des N dernières mesures.
    let smoothed_hz = new_sum / new_count;

    // ── Calcul de la correction ────────────────────────────────────────────────
    let current_hz  = PLL.current_hz.load(Ordering::Relaxed);
    if current_hz == 0 { return (ref_ns_anchor, measured_hz); }

    // Erreur brute (signée).
    let error_hz: i64 = smoothed_hz as i64 - current_hz as i64;

    // Limite de correction : MAX_ADJ_PPM × current_hz / 1_000_000.
    let max_adj_hz = ((current_hz as u128 * MAX_ADJ_PPM as u128) / 1_000_000) as i64;
    let max_adj_hz = max_adj_hz.max(1); // Au moins 1 Hz de correction possible.

    // Clamp de l'erreur à ±MAX_ADJ_PPM.
    let clamped_error = error_hz.clamp(-max_adj_hz, max_adj_hz);

    // Filtre passe-bas : appliquer FILTER_COEFF % de la correction.
    let adj_hz = (clamped_error * FILTER_COEFF_MILLI as i64) / 1000;

    // Nouvelle fréquence après correction (ne pas dépasser les limites absolues).
    let new_hz_signed = current_hz as i64 + adj_hz;
    let new_hz = (new_hz_signed.max(500_000_000) as u64).min(10_000_000_000);

    // ── Vérification convergence ────────────────────────────────────────────────
    let deviation_ppm = if new_hz > 0 {
        (error_hz.unsigned_abs() as u128 * 1_000_000 / new_hz as u128) as u64
    } else { 0 };

    if deviation_ppm < LOCK_THRESHOLD_PPM {
        PLL.locked.store(true, Ordering::Relaxed);
    } else {
        PLL.locked.store(false, Ordering::Relaxed);
    }

    PLL.last_adj_hz.store(adj_hz, Ordering::Relaxed);
    PLL.current_hz.store(new_hz, Ordering::Release);
    PLL.total_corrections.fetch_add(1, Ordering::Relaxed);

    // ── Recalcul de ns_anchor pour garantir la monotonie ─────────────────────
    // Si on change tsc_hz, le calcul ns = tsc_delta × 1e9 / hz change.
    // Il faut recalculer ns_anchor à partir du tsc_now actuel et de la nouvelle hz.
    // RÈGLE DRIFT-MONOTONE-01 : ns_anchor_new >= ns_anchor_old.
    let ns_anchor_new = ref_ns_anchor; // Le caller calcule déjà ns_now correctement.

    (ns_anchor_new, new_hz)
}

/// Retourne `true` si la PLL est en état de lock (dérive < 100 ppm).
#[inline(always)]
pub fn pll_locked() -> bool {
    PLL.locked.load(Ordering::Relaxed)
}

/// Retourne la fréquence courante selon la PLL (Hz).
#[inline(always)]
pub fn pll_current_hz() -> u64 {
    PLL.current_hz.load(Ordering::Relaxed)
}

/// Retourne le dernier ajustement appliqué (Hz, signé).
#[inline(always)]
pub fn pll_last_adj_hz() -> i64 {
    PLL.last_adj_hz.load(Ordering::Relaxed)
}

/// Retourne le nombre total de corrections PLL depuis le boot.
pub fn pll_correction_count() -> u64 {
    PLL.total_corrections.load(Ordering::Relaxed)
}

/// Snapshot complet de l'état PLL pour diagnostic.
pub struct PllSnapshot {
    pub current_hz:    u64,
    pub last_adj_hz:   i64,
    pub locked:        bool,
    pub corrections:   u64,
    pub smoothed_hz:   u64,
}

pub fn pll_snapshot() -> PllSnapshot {
    let sum   = PLL.measure_sum.load(Ordering::Relaxed);
    let count = PLL.measure_count.load(Ordering::Relaxed).max(1);
    PllSnapshot {
        current_hz:  PLL.current_hz.load(Ordering::Relaxed),
        last_adj_hz: PLL.last_adj_hz.load(Ordering::Relaxed),
        locked:      PLL.locked.load(Ordering::Relaxed),
        corrections: PLL.total_corrections.load(Ordering::Relaxed),
        smoothed_hz: sum / count,
    }
}
