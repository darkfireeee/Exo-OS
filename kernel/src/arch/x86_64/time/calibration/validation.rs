// kernel/src/arch/x86_64/time/calibration/validation.rs
//
// ════════════════════════════════════════════════════════════════════════════════
// Validation croisée et filtrage statistique de la calibration TSC
// ════════════════════════════════════════════════════════════════════════════════
//
// ## Rôle
//   Valider qu'une fréquence TSC mesurée (via HPET/PM Timer/PIT) est cohérente
//   avec la valeur nominale fabricant (CPUID 0x15) et les limites physiques.
//
// ## Niveaux de validation
//
//   Niveau 1 — Plage absolue [500 MHz, 10 GHz]
//     Toute fréquence hors de cette plage est rejetée sans discussion.
//
//   Niveau 2 — Cross-check CPUID 0x15
//     Si CPUID 0x15 est disponible, comparer avec la mesure :
//       Écart ≤ 2%    → EXCELLENT : mesure et CPUID concordent bien
//       Écart ≤ 5%    → BON : concordance acceptable (QEMU/VM, température élevée)
//       Écart ≤ 10%   → AVERTISSEMENT : mesure acceptée mais loguée
//       Écart 10-20%  → DÉGRADÉ : probable drift thermique ou ECX=0 fallback crystal
//       Écart > 20%   → HARDWARE SUSPECT : mesure peut-être corrompue
//                       Utiliser CPUID nominal si disponible
//
//   Niveau 3 — Cohérence successive (multi-mesures)
//     Comparer avec la dernière valeur validée : variation > 1% → avertissement.
//     Évite les sauts brutaux de fréquence qui causeraient des sauts ktime.
//
//   Niveau 4 — Cohérence avec l'historique global
//     Sur N mesures, calculer l'écart-type. Si > 1% → instabilité TSC détectée.
//
// ## Règles
//   RÈGLE CAL-VALIDATE-01 : cross-check avec CPUID 0x15, seuils 10%/20%
//   RÈGLE CAL-VALIDATE-02 : variation successive > 1% → warning
//   RÈGLE CAL-VALIDATE-03 : historique N mesures, écart-type > 1% → instabilité
//   RÈGLE CAL-VALIDATE-04 : plage absolue [500 MHz, 10 GHz] — rejet immédiat
// ════════════════════════════════════════════════════════════════════════════════

use super::cpuid_nominal;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ── Constantes ────────────────────────────────────────────────────────────────

/// Fréquence TSC minimum acceptable (500 MHz).
pub const TSC_FREQ_MIN_HZ: u64 = 500_000_000;
/// Fréquence TSC maximum acceptable (10 GHz).
pub const TSC_FREQ_MAX_HZ: u64 = 10_000_000_000;

/// Seuil "excellent" : écart < 2 % (permillage × 10 = cent-centième).
const THRESHOLD_EXCELLENT_PCT_X100: u128 = 200; // 2 %
/// Seuil "bon" : écart < 5 %.
const THRESHOLD_GOOD_PCT_X100: u128 = 500; // 5 %
/// Seuil "avertissement" : écart < 10 %.
const THRESHOLD_WARN_PCT_X100: u128 = 1_000; // 10 %
/// Seuil "dégradé" : écart < 20 %.
const THRESHOLD_DEGRADED_PCT_X100: u128 = 2_000; // 20 %

/// Taille de l'historique de mesures conservé pour le calcul de l'écart-type.
const HISTORY_SIZE: usize = 8;

/// Variation maximale successive acceptable (1 %).
const MAX_SUCCESSIVE_VARIATION_PCT_X100: u128 = 100; // 1 %

// ── Types ──────────────────────────────────────────────────────────────────────

/// Résultat détaillé de la validation d'une fréquence TSC mesurée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationResult {
    /// Concordance excellente (<2% d'écart avec CPUID). Confiance totale.
    Excellent(u64),
    /// Concordance bonne (<5% d'écart). Confiance normale.
    Good(u64),
    /// Divergence modérée (<10%). Mesure acceptée, warning émis.
    Warning {
        measured: u64,
        cpuid: u64,
        pct_x100: u64,
    },
    /// Divergence significative (10-20%). Mesure acceptée mais dégradée.
    Degraded {
        measured: u64,
        cpuid: u64,
        pct_x100: u64,
    },
    /// Divergence critique (>20%). Mesure rejetée, CPUID préféré si disponible.
    Rejected {
        measured: u64,
        cpuid: u64,
        pct_x100: u64,
    },
    /// CPUID 0x15 non disponible → validation impossible, mesure retournée telle quelle.
    NoCpuid(u64),
    /// Valeur hors plage absolue [500 MHz, 10 GHz].
    OutOfRange(u64),
    /// Variation successive trop importante (>1% par rapport à la dernière mesure).
    SuccessiveJump {
        new_hz: u64,
        prev_hz: u64,
        pct_x100: u64,
    },
}

impl ValidationResult {
    /// Retourne la fréquence recommandée (None si rejetée).
    pub fn accepted_hz(self) -> Option<u64> {
        match self {
            ValidationResult::Excellent(hz) => Some(hz),
            ValidationResult::Good(hz) => Some(hz),
            ValidationResult::Warning { measured, .. } => Some(measured),
            ValidationResult::Degraded {
                measured,
                cpuid: _,
                pct_x100: _,
            } => Some(measured),
            ValidationResult::Rejected {
                measured: _, cpuid, ..
            } => {
                // Préférer CPUID si la mesure est hors plage.
                if cpuid > 0 {
                    Some(cpuid)
                } else {
                    None
                }
            }
            ValidationResult::NoCpuid(hz) => Some(hz),
            ValidationResult::OutOfRange(_) => None,
            ValidationResult::SuccessiveJump { new_hz, .. } => Some(new_hz), // accepter malgré le saut
        }
    }

    /// Indique si la mesure est fiable (Excellent ou Good).
    pub fn is_high_confidence(self) -> bool {
        matches!(
            self,
            ValidationResult::Excellent(_)
                | ValidationResult::Good(_)
                | ValidationResult::NoCpuid(_)
        )
    }

    /// Niveau de confiance de 0 (rejet total) à 100 (parfait).
    pub fn confidence(self) -> u8 {
        match self {
            ValidationResult::Excellent(_) => 100,
            ValidationResult::Good(_) => 90,
            ValidationResult::Warning { .. } => 70,
            ValidationResult::Degraded { .. } => 50,
            ValidationResult::Rejected { .. } => 10,
            ValidationResult::NoCpuid(_) => 80, // Pas de référence, mais mesure directe OK.
            ValidationResult::OutOfRange(_) => 0,
            ValidationResult::SuccessiveJump { .. } => 60,
        }
    }

    /// Retourne un label court pour les logs port 0xE9.
    pub fn label(self) -> &'static str {
        match self {
            ValidationResult::Excellent(_) => "EXCE",
            ValidationResult::Good(_) => "GOOD",
            ValidationResult::Warning { .. } => "WARN",
            ValidationResult::Degraded { .. } => "DEGR",
            ValidationResult::Rejected { .. } => "RJCT",
            ValidationResult::NoCpuid(_) => "NOCI",
            ValidationResult::OutOfRange(_) => "ORAN",
            ValidationResult::SuccessiveJump { .. } => "JUMP",
        }
    }
}

// ── Historique de calibration ──────────────────────────────────────────────────

/// Historique des dernières mesures de fréquence TSC (FIFO circulaire).
static HISTORY: [AtomicU64; HISTORY_SIZE] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; HISTORY_SIZE]
};
static HISTORY_IDX: AtomicU32 = AtomicU32::new(0);
static HISTORY_COUNT: AtomicU32 = AtomicU32::new(0);
/// Dernière valeur validée et acceptée (pour contrôle successif).
static LAST_ACCEPTED_HZ: AtomicU64 = AtomicU64::new(0);
/// Nombre total de validations effectuées depuis le boot.
static VALIDATION_COUNT: AtomicU64 = AtomicU64::new(0);
/// Nombre de validations dégradées (Degraded + Rejected).
static DEGRADED_COUNT: AtomicU64 = AtomicU64::new(0);

// ── API principale ─────────────────────────────────────────────────────────────

/// Validation complète d'une fréquence TSC mesurée.
///
/// Effectue dans l'ordre :
///   1. Vérification plage absolue (RÈGLE CAL-VALIDATE-04)
///   2. Cross-check CPUID 0x15 (RÈGLE CAL-VALIDATE-01)
///   3. Vérification variation successive (RÈGLE CAL-VALIDATE-02)
///   4. Mise à jour de l'historique
///
/// Retourne `None` si la mesure est rejetée (out-of-range ou erreur).
pub fn cross_check(measured_hz: u64) -> Option<u64> {
    VALIDATION_COUNT.fetch_add(1, Ordering::Relaxed);

    let result = validate_full(measured_hz);

    // Loguer le résultat sur port 0xE9 si la confiance est basse.
    if result.confidence() < 70 {
        DEGRADED_COUNT.fetch_add(1, Ordering::Relaxed);
        log_validation_result(measured_hz, &result);
    }

    match result.accepted_hz() {
        Some(hz) => {
            push_history(hz);
            LAST_ACCEPTED_HZ.store(hz, Ordering::Relaxed);
            Some(hz)
        }
        None => None,
    }
}

/// Vérifie que la fréquence est dans la plage physique raisonnable.
#[inline(always)]
pub fn hz_in_range(hz: u64) -> bool {
    hz >= TSC_FREQ_MIN_HZ && hz <= TSC_FREQ_MAX_HZ
}

/// Validation complète retournant le `ValidationResult` détaillé.
pub fn validate_full(measured_hz: u64) -> ValidationResult {
    // ── Niveau 1 : plage absolue ───────────────────────────────────────────────
    if !hz_in_range(measured_hz) {
        return ValidationResult::OutOfRange(measured_hz);
    }

    // ── Niveau 3 : vérification variation successive ───────────────────────────
    let prev = LAST_ACCEPTED_HZ.load(Ordering::Relaxed);
    if prev > 0 {
        let successive_pct = percent_x100(measured_hz, prev);
        if successive_pct > MAX_SUCCESSIVE_VARIATION_PCT_X100 as u64 {
            // Saut important → loguer mais accepter (peut être normal après re-calibration forcée).
            return ValidationResult::SuccessiveJump {
                new_hz: measured_hz,
                prev_hz: prev,
                pct_x100: successive_pct,
            };
        }
    }

    // ── Niveau 2 : cross-check CPUID 0x15 ─────────────────────────────────────
    // RÈGLE CAL-VALIDATE-01 : comparaison avec la valeur nominale fabricant.
    let cpuid_hz = match cpuid_nominal::cpuid_tsc_hz() {
        Some(hz) if hz > 0 => hz,
        _ => {
            // CPUID 0x15 non disponible — pas de cross-check possible.
            return ValidationResult::NoCpuid(measured_hz);
        }
    };

    let pct_x100 = percent_x100_u128(measured_hz as u128, cpuid_hz as u128);

    if pct_x100 <= THRESHOLD_EXCELLENT_PCT_X100 {
        ValidationResult::Excellent(measured_hz)
    } else if pct_x100 <= THRESHOLD_GOOD_PCT_X100 {
        ValidationResult::Good(measured_hz)
    } else if pct_x100 <= THRESHOLD_WARN_PCT_X100 {
        ValidationResult::Warning {
            measured: measured_hz,
            cpuid: cpuid_hz,
            pct_x100: pct_x100 as u64,
        }
    } else if pct_x100 <= THRESHOLD_DEGRADED_PCT_X100 {
        ValidationResult::Degraded {
            measured: measured_hz,
            cpuid: cpuid_hz,
            pct_x100: pct_x100 as u64,
        }
    } else {
        ValidationResult::Rejected {
            measured: measured_hz,
            cpuid: cpuid_hz,
            pct_x100: pct_x100 as u64,
        }
    }
}

/// Variante simple retournant seulement le résultat classique (pour calibration/mod.rs).
pub fn validate(measured_hz: u64) -> ValidationResult {
    validate_full(measured_hz)
}

// ── Analyse statistique de l'historique ───────────────────────────────────────

/// Calcule la moyenne des mesures dans l'historique.
pub fn history_mean_hz() -> u64 {
    let count = HISTORY_COUNT.load(Ordering::Relaxed) as usize;
    if count == 0 {
        return 0;
    }
    let n = count.min(HISTORY_SIZE);
    let mut sum = 0u128;
    for i in 0..n {
        sum += HISTORY[i].load(Ordering::Relaxed) as u128;
    }
    (sum / n as u128) as u64
}

/// Calcule la variance normalisée des mesures (en ppm²).
/// Un résultat > 10_000 ppm² (~100 ppm std dev) indique une instabilité TSC.
pub fn history_variance_ppm2() -> u64 {
    let count = HISTORY_COUNT.load(Ordering::Relaxed) as usize;
    let n = count.min(HISTORY_SIZE);
    if n < 2 {
        return 0;
    }

    let mean = history_mean_hz();
    if mean == 0 {
        return 0;
    }

    let mut sum_sq_ppm: u128 = 0;
    for i in 0..n {
        let val = HISTORY[i].load(Ordering::Relaxed);
        // déviation en ppm = |val - mean| × 1_000_000 / mean
        let dev_ppm = if val >= mean {
            ((val - mean) as u128 * 1_000_000) / mean as u128
        } else {
            ((mean - val) as u128 * 1_000_000) / mean as u128
        };
        sum_sq_ppm = sum_sq_ppm.saturating_add(dev_ppm * dev_ppm);
    }
    (sum_sq_ppm / n as u128) as u64
}

/// Retourne `true` si l'historique indique un TSC instable (variance > seuil).
/// Seuil : variance > 10_000 ppm² (std dev ≈ 100 ppm ≈ 300 Hz à 3 GHz).
pub fn tsc_is_unstable() -> bool {
    history_variance_ppm2() > 10_000
}

/// Snapshot de l'état de validation pour diagnostics.
pub struct ValidationSnapshot {
    pub validation_count: u64,
    pub degraded_count: u64,
    pub last_accepted_hz: u64,
    pub history_mean_hz: u64,
    pub variance_ppm2: u64,
    pub unstable: bool,
    pub cpuid_available: bool,
    pub cpuid_hz: u64,
}

pub fn validation_snapshot() -> ValidationSnapshot {
    ValidationSnapshot {
        validation_count: VALIDATION_COUNT.load(Ordering::Relaxed),
        degraded_count: DEGRADED_COUNT.load(Ordering::Relaxed),
        last_accepted_hz: LAST_ACCEPTED_HZ.load(Ordering::Relaxed),
        history_mean_hz: history_mean_hz(),
        variance_ppm2: history_variance_ppm2(),
        unstable: tsc_is_unstable(),
        cpuid_available: cpuid_nominal::cpuid_leaf15_available(),
        cpuid_hz: cpuid_nominal::cpuid_tsc_hz().unwrap_or(0),
    }
}

// ── Recommandation finale ──────────────────────────────────────────────────────

/// Retourne la meilleure fréquence TSC disponible en combinant mesure + CPUID.
///
/// Logique de fusion conservative :
///   - Si mesure validée EXCELLENT/GOOD → utiliser la mesure (plus précise que CPUID).
///   - Si mesure WARN/DEGRADED + CPUID disponible → moyenne pondérée 70/30.
///   - Si mesure REJECTED + CPUID disponible → CPUID uniquement.
///   - Si aucun CPUID → mesure directement.
///   - Résultat toujours dans [500 MHz, 10 GHz].
pub fn best_frequency(measured_hz: u64) -> Option<u64> {
    let result = validate_full(measured_hz);
    let cpuid_hz = cpuid_nominal::cpuid_tsc_hz().unwrap_or(0);

    let hz = match result {
        ValidationResult::Excellent(hz) | ValidationResult::Good(hz) => hz,
        ValidationResult::NoCpuid(hz) => hz,
        ValidationResult::Warning { measured, .. } => {
            // Trust la mesure pour le Warning (CPUID nominal peut être décalé en VM).
            measured
        }
        ValidationResult::Degraded {
            measured, cpuid, ..
        } => {
            if cpuid > 0 {
                // Moyenne pondérée 70% mesure + 30% CPUID.
                let weighted = (measured as u128 * 70 + cpuid as u128 * 30) / 100;
                weighted as u64
            } else {
                measured
            }
        }
        ValidationResult::Rejected { cpuid, .. } => {
            if cpuid > 0 {
                cpuid
            } else {
                return None;
            }
        }
        ValidationResult::OutOfRange(_) => return None,
        ValidationResult::SuccessiveJump { new_hz, .. } => {
            // Saut successif : utiliser la mesure mais vérifier avec CPUID.
            if cpuid_hz > 0 {
                let pct = percent_x100_u128(new_hz as u128, cpuid_hz as u128);
                if pct > THRESHOLD_DEGRADED_PCT_X100 {
                    // Trop divergent même avec le saut → CPUID.
                    cpuid_hz
                } else {
                    new_hz
                }
            } else {
                new_hz
            }
        }
    };

    if hz_in_range(hz) {
        Some(hz)
    } else {
        None
    }
}

// ── Utilitaires internes ───────────────────────────────────────────────────────

/// Calcule l'écart en centièmes de % entre deux fréquences (|a - b| / b × 10000).
fn percent_x100_u128(a: u128, b: u128) -> u128 {
    if b == 0 {
        return 10_000;
    }
    let diff = if a > b { a - b } else { b - a };
    diff.saturating_mul(10_000) / b
}

/// Version u64 pour les comparaisons critiques.
fn percent_x100(a: u64, b: u64) -> u64 {
    if b == 0 {
        return 10_000;
    }
    let diff = if a > b { a - b } else { b - a };
    ((diff as u128).saturating_mul(10_000) / b as u128) as u64
}

/// Ajoute une valeur dans l'historique circulaire.
fn push_history(hz: u64) {
    let idx = (HISTORY_IDX.fetch_add(1, Ordering::Relaxed) as usize) % HISTORY_SIZE;
    HISTORY[idx].store(hz, Ordering::Relaxed);
    // Incrémenter le count jusqu'à HISTORY_SIZE.
    let prev = HISTORY_COUNT.load(Ordering::Relaxed) as usize;
    if prev < HISTORY_SIZE {
        HISTORY_COUNT.store((prev + 1) as u32, Ordering::Relaxed);
    }
}

/// Émet un message de diagnostic sur le port 0xE9 si le résultat est dégradé.
fn log_validation_result(measured_hz: u64, result: &ValidationResult) {
    // Format : "[VAL:XXXX hz=XXXXXXXX]\n"
    unsafe fn out(b: u8) {
        unsafe {
            core::arch::asm!("out 0xe9, al", in("al") b, options(nomem, nostack));
        }
    }
    let label = result.label();
    // SAFETY: port 0xE9 = canal debug QEMU, aucun effet si non disponible.
    unsafe {
        for &b in b"[VAL:" {
            out(b);
        }
        for &b in label.as_bytes() {
            out(b);
        }
        out(b' ');
        // Écrire measured_hz en hex 16 chiffres.
        let v = measured_hz;
        for shift in (0..64usize).step_by(4).rev() {
            let nib = ((v >> (60 - shift)) & 0xF) as u8;
            out(if nib < 10 {
                b'0' + nib
            } else {
                b'a' + nib - 10
            });
        }
        out(b']');
        out(b'\n');
    }
}
