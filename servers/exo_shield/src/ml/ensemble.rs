//! Fusion ensembliste MLP + Isolation Forest + Markov pour ExoShield NGAV.
//!
//! Les trois modèles tournent en "parallèle" (séquentiellement, no_std) et leurs
//! scores sont combinés par pondération linéaire :
//!
//!   combined = w_mlp × P(Malicious)_MLP
//!            + w_if  × IF_score
//!            + w_mk  × normalize(Markov_surprise)
//!
//! Poids par défaut : MLP=0.45, IF=0.35, Markov=0.20 (somme = 65536 en Q16.16).
//!
//! Le classifieur alimente automatiquement la calibration IF sur les samples
//! bénins (apprentissage online du "normal").

use core::sync::atomic::{AtomicBool, Ordering};

use super::features::{FeatureVector, FEATURE_COUNT};
use super::inference::Classification;
use super::iforest;
use super::markov;
use super::mlp;
use super::trained_weights;

/// FIX-F10 : bornes basses de normalisation (toutes à 0). Le max par feature vient
/// de `trained_weights::FEATURE_MAX`. Utilisé pour mapper les features brutes
/// [0,99] vers Q16.16 [0,1] avant le MLP.
const FEAT_MIN_ZERO: [i32; FEATURE_COUNT] = [0; FEATURE_COUNT];

// ── Seuils de classification ──────────────────────────────────────────────────

/// Combined ≥ 0.60 → Malicious
const THRESH_MALICIOUS: i32 = 39_322;
/// Combined ≥ 0.33 → Suspicious
const THRESH_SUSPICIOUS: i32 = 21_845;

// ── Poids (Q16.16, somme = 65536) ────────────────────────────────────────────

const W_MLP: i32 = 29_491;   // ≈ 0.45
const W_IF: i32 = 22_938;    // ≈ 0.35
const W_MARKOV: i32 = 13_107; // ≈ 0.20

const _: () = assert!(W_MLP + W_IF + W_MARKOV == 65_536);

// ── Résultat ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct EnsembleResult {
    /// P(Malicious) du MLP en Q16.16 [0, 65536]
    pub mlp_prob: i32,
    /// Score IF en Q16.16 [0, 65536]
    pub if_score: i32,
    /// Surprise Markov normalisée en Q16.16 [0, 65536]
    pub markov_score: i32,
    /// Score combiné en Q16.16 [0, 65536]
    pub combined: i32,
    /// Classification finale
    pub classification: Classification,
    /// Score en [0, 1000] compatible compute_threat_score()
    pub threat_score: u32,
}

impl EnsembleResult {
    pub const fn zero() -> Self {
        Self {
            mlp_prob: 0,
            if_score: 0,
            markov_score: 0,
            combined: 0,
            classification: Classification::Benign,
            threat_score: 0,
        }
    }
}

// ── Classification depuis le score combiné ────────────────────────────────────

fn classify_combined(combined: i32) -> Classification {
    if combined >= THRESH_MALICIOUS {
        Classification::Malicious
    } else if combined >= THRESH_SUSPICIOUS {
        Classification::Suspicious
    } else {
        Classification::Benign
    }
}

// ── Inférence ─────────────────────────────────────────────────────────────────

/// Fusionne les 3 modèles sur le feature vector.
///
/// - `pid` : identifiant de processus (état Markov par-PID)
/// - `event_type_raw` : octet EventType behavioral (0..8 pour Markov)
/// - `fv` : vecteur de features brutes (pas besoin d'être normalisé)
pub fn ensemble_classify(
    pid: u32,
    event_type_raw: u8,
    fv: &FeatureVector,
) -> EnsembleResult {
    // FIX-F10 : normaliser les features brutes [0,99] -> Q16.16 [0,1] AVANT le MLP.
    // Sans cela, le forward Q16.16 interprète 50 comme ~0.0008 → MLP inerte
    // (sortie ≈0.5 quelle que soit l'entrée). L'IF et Markov conservent les valeurs
    // BRUTES (leurs seuils sont calibrés sur l'échelle brute 0..99).
    let mut fv_mlp = *fv;
    fv_mlp.normalise_minmax(&FEAT_MIN_ZERO, &trained_weights::FEATURE_MAX);
    let (mlp_prob, _) = mlp::mlp_infer(&fv_mlp);
    let if_score = iforest::iforest_score(fv);
    let markov_score = markov::markov_observe(pid, event_type_raw);

    // Combinaison pondérée en Q16.16
    let combined = ((W_MLP as i64 * mlp_prob as i64
        + W_IF as i64 * if_score as i64
        + W_MARKOV as i64 * markov_score as i64)
        >> 16)
        .clamp(0, 65_536) as i32;

    let classification = classify_combined(combined);
    let threat_score = ((combined as u64 * 1_000) >> 16) as u32;

    // Calibration IF : feed les samples bénins pour améliorer la baseline
    if classification == Classification::Benign {
        iforest::iforest_observe_normal(fv);
    }

    EnsembleResult {
        mlp_prob,
        if_score,
        markov_score,
        combined,
        classification,
        threat_score,
    }
}

// ── Initialisation ────────────────────────────────────────────────────────────

static ENSEMBLE_READY: AtomicBool = AtomicBool::new(false);

/// Initialise tous les composants ML. Appeler une seule fois au démarrage.
pub fn ensemble_init(seed: u32) {
    mlp::mlp_init(seed);
    iforest::iforest_init(seed ^ 0x5E_C1_17);
    markov::markov_init();
    // FIX-F4/F10 : charger les poids ENTRAÎNÉS (chemin authentifié checksum+version).
    // En cas d'échec d'intégrité, on conserve les poids seedés (dégradation
    // gracieuse, jamais de panic) — le NGAV reste opérationnel.
    let _mlp_trained = mlp::mlp_load_trained();
    let _if_trained = iforest::iforest_load_trained();
    ENSEMBLE_READY.store(true, Ordering::Release);
}

pub fn ensemble_is_ready() -> bool {
    ENSEMBLE_READY.load(Ordering::Acquire)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::features::FEATURE_COUNT;

    fn init() {
        ensemble_init(42);
    }

    #[test]
    fn ensemble_all_outputs_bounded() {
        init();
        let fv = FeatureVector::from_raw([50i32; FEATURE_COUNT]);
        let r = ensemble_classify(1, 0, &fv);
        assert!(r.mlp_prob >= 0 && r.mlp_prob <= 1 << 16, "mlp={}", r.mlp_prob);
        assert!(r.if_score >= 0 && r.if_score <= 1 << 16, "if={}", r.if_score);
        assert!(r.markov_score >= 0 && r.markov_score <= 1 << 16, "mk={}", r.markov_score);
        assert!(r.combined >= 0 && r.combined <= 1 << 16, "combined={}", r.combined);
        assert!(r.threat_score <= 1000, "ts={}", r.threat_score);
    }

    #[test]
    fn ensemble_zero_vector_not_unknown() {
        init();
        let fv = FeatureVector::zero();
        let r = ensemble_classify(10, 0, &fv);
        assert_ne!(r.classification, Classification::Unknown);
        assert!(r.combined >= 0 && r.combined <= 1 << 16);
    }

    #[test]
    fn ensemble_weights_sum_exactly_one_q16() {
        assert_eq!(W_MLP + W_IF + W_MARKOV, 65_536);
    }

    #[test]
    fn ensemble_combined_math_half_inputs() {
        // Si tous les scores sont à 0.5 → combined ≈ 0.5
        let mlp: i32 = 32_768;
        let iif: i32 = 32_768;
        let mk: i32 = 32_768;
        let c = ((W_MLP as i64 * mlp as i64
            + W_IF as i64 * iif as i64
            + W_MARKOV as i64 * mk as i64)
            >> 16)
            .clamp(0, 65_536) as i32;
        // Doit être proche de 32768
        assert!(c >= 32_000 && c <= 33_000, "combined={}", c);
    }

    #[test]
    fn ensemble_threat_score_matches_combined() {
        let c = 32_768i32;
        let ts = ((c as u64 * 1_000) >> 16) as u32;
        assert!(ts >= 499 && ts <= 501, "ts={}", ts);
    }

    #[test]
    fn ensemble_max_dangerous_bounded() {
        init();
        let mut d = [0i32; FEATURE_COUNT];
        d[30] = 65_536; // ptrace
        d[12] = 65_536; // priv_escalation
        d[27] = 65_536; // raw_socket
        d[28] = 65_536; // chroot
        d[29] = 65_536; // clone_namespace
        let fv = FeatureVector::from_raw(d);
        let r = ensemble_classify(99, 5, &fv);
        assert!(r.combined >= 0 && r.combined <= 1 << 16);
        assert!(r.threat_score <= 1000);
    }

    #[test]
    fn ensemble_repeated_benign_calibrates_iforest() {
        init();
        let fv = FeatureVector::from_raw([5i32; FEATURE_COUNT]);
        // Plusieurs appels sur pattern bénin → calibration IF augmente
        for _ in 0..20 {
            ensemble_classify(5, 0, &fv);
        }
        let count = iforest::iforest_calibration_count();
        assert!(count > 0, "IF devrait avoir des observations de calibration");
    }

    #[test]
    fn ensemble_classif_thresholds_consistent() {
        // Vérifie que les seuils ne se chevauchent pas
        assert!(THRESH_SUSPICIOUS < THRESH_MALICIOUS);
        assert!(THRESH_SUSPICIOUS > 0);
        assert!(THRESH_MALICIOUS < (1 << 16));
    }

    /// FIX-F3/F4/F10 — preuve end-to-end :
    /// 1. Les poids ENTRAÎNÉS se chargent (checksum Python↔Rust OK, version=2).
    /// 2. Le MLP n'est plus INERTE : un événement malveillant produit un
    ///    P(malicious) strictement > celui d'un événement bénin (grâce à la
    ///    normalisation F10 ; sans elle les deux donnaient ≈0.5).
    #[test]
    fn ensemble_trained_weights_loaded_and_mlp_discriminates() {
        init();
        // (1) checksum + version : si le checksum ne matchait pas, mlp_load_trained
        // renverrait false et la version resterait à 1 (poids seedés).
        assert_eq!(
            mlp::mlp_version(),
            trained_weights::TRAINED_MLP_VERSION,
            "poids entraînés NON chargés — checksum Python/Rust divergent ?"
        );

        // (2) discrimination réelle (anti-inertie F10).
        let mut benign = [0i32; FEATURE_COUNT];
        benign[0] = 5; // syscall_rate faible
        let mut malicious = [0i32; FEATURE_COUNT];
        malicious[12] = 90; // priv_escalation
        malicious[13] = 90; // denied_syscall
        malicious[27] = 90; // raw_socket
        malicious[30] = 90; // ptrace
        let rb = ensemble_classify(100, 0, &FeatureVector::from_raw(benign));
        let rm = ensemble_classify(101, 0, &FeatureVector::from_raw(malicious));
        assert!(
            rm.mlp_prob > rb.mlp_prob,
            "MLP inerte : P(mal) malicious={} <= benign={}",
            rm.mlp_prob,
            rb.mlp_prob
        );
        assert!(
            rm.combined > rb.combined,
            "ensemble ne discrimine pas : combined malicious={} <= benign={}",
            rm.combined,
            rb.combined
        );
    }
}
