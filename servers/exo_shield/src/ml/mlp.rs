//! Deep MLP 32→128→64→4 pour le NGAV ExoShield.
//!
//! Architecture : Input[32] → Dense[128] → LeakyReLU
//!                           → Dense[64]  → LeakyReLU
//!                           → Dense[4]   → Sigmoid
//!
//! Toute l'arithmétique est en Q16.16 (1.0 = 65536).
//! Les poids (~51 KB) sont en BSS statique via `spin::Mutex<MlpWeights>`.
//! L'init est domain-informed : les features dangereuses (ptrace, raw_socket…)
//! ont un biais positif vers out[2] (P(Malicious)).

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::features::{FeatureVector, FEATURE_COUNT};
use super::inference::Classification;

// ── Dimensions ────────────────────────────────────────────────────────────────

pub const MLP_H1: usize = 128;
pub const MLP_H2: usize = 64;
pub const MLP_OUT: usize = 4;

/// Index de sortie pour P(Malicious)
pub const OUT_MALICIOUS: usize = 2;

/// Seuil de classification Malicious (0.5 en Q16.16)
pub const MLP_DEFAULT_THRESHOLD: i32 = 32_768;

// ── Activations Q16.16 ────────────────────────────────────────────────────────

#[inline(always)]
fn leaky_relu(x: i32) -> i32 {
    if x >= 0 { x } else { ((x as i64 * 655) >> 16) as i32 }
}

/// Approximation linéaire par morceaux de sigmoid, clamped sur [-4, 4].
#[inline(always)]
fn sigmoid(x: i32) -> i32 {
    const FP_ONE: i32 = 1 << 16;
    const HALF: i32 = 1 << 15;
    if x >= (4 << 16) { return FP_ONE; }
    if x <= -(4 << 16) { return 0; }
    (HALF + ((x as i64) / 8) as i32).clamp(0, FP_ONE)
}

// ── LCG ──────────────────────────────────────────────────────────────────────

#[inline(always)]
fn lcg_next(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    *state
}

// ── Poids ─────────────────────────────────────────────────────────────────────

/// Poids complets du MLP 32→128→64→4. ~51 KB, destiné au BSS statique.
pub struct MlpWeights {
    /// Layer 1 : FEATURE_COUNT×MLP_H1, row-major
    pub w1: [i32; FEATURE_COUNT * MLP_H1],
    pub b1: [i32; MLP_H1],
    /// Layer 2 : MLP_H1×MLP_H2, row-major
    pub w2: [i32; MLP_H1 * MLP_H2],
    pub b2: [i32; MLP_H2],
    /// Layer 3 : MLP_H2×MLP_OUT, row-major
    pub w3: [i32; MLP_H2 * MLP_OUT],
    pub b3: [i32; MLP_OUT],
    pub version: u32,
}

impl MlpWeights {
    /// Zéro-init pour placement BSS. Appeler `init_domain_seeded()` avant usage.
    pub const fn zero() -> Self {
        Self {
            w1: [0i32; FEATURE_COUNT * MLP_H1],
            b1: [0i32; MLP_H1],
            w2: [0i32; MLP_H1 * MLP_H2],
            b2: [0i32; MLP_H2],
            w3: [0i32; MLP_H2 * MLP_OUT],
            b3: [0i32; MLP_OUT],
            version: 0,
        }
    }

    /// Initialisation domain-informed via LCG.
    ///
    /// W1, W2 : petites valeurs Xavier-like aléatoires.
    /// W3 (couche finale) : biais pour que les neurones H2 associés aux features
    /// dangereuses poussent vers out[2] (Malicious) et loin de out[0] (Benign).
    pub fn init_domain_seeded(&mut self, seed: u32) {
        let mut state = seed;

        // Layer 1 : σ Xavier ≈ 1/√32 ≈ 0.177 → 11600 en Q16.16
        for v in self.w1.iter_mut() {
            let r = lcg_next(&mut state);
            *v = ((r >> 16) as i32 % 11_600) - 5_800;
        }
        for v in self.b1.iter_mut() { *v = 0; }

        // Layer 2 : σ ≈ 1/√128 ≈ 0.088 → 5800 en Q16.16
        for v in self.w2.iter_mut() {
            let r = lcg_next(&mut state);
            *v = ((r >> 16) as i32 % 5_800) - 2_900;
        }
        for v in self.b2.iter_mut() { *v = 0; }

        // Layer 3 : 64→4, avec biais domain (neurones H2[0..16] → Malicious)
        for j in 0..MLP_OUT {
            for i in 0..MLP_H2 {
                let r = lcg_next(&mut state);
                let base = ((r >> 16) as i32 % 4_000) - 2_000;
                let domain = if j == OUT_MALICIOUS && i < 16 {
                    2_000i32  // fort biais positif vers Malicious
                } else if j == 0 && i < 16 {
                    -1_000i32 // biais négatif vers Benign pour neurones dangereux
                } else if j == 0 && i >= 48 {
                    1_500i32  // biais positif vers Benign pour neurones "sûrs"
                } else {
                    0i32
                };
                self.w3[i * MLP_OUT + j] = base.saturating_add(domain);
            }
        }
        // Léger biais de base : la plupart des processus sont bénins
        self.b3[0] = 3_276;   // +0.05 Benign
        self.b3[2] = -3_276;  // -0.05 Malicious
        self.version = 1;
    }

    /// Forward pass : input[32] → output[4] en Q16.16.
    pub fn forward(&self, input: &FeatureVector) -> [i32; MLP_OUT] {
        // Layer 1 : 32 → 128
        let mut h1 = [0i32; MLP_H1];
        for j in 0..MLP_H1 {
            let mut acc: i64 = self.b1[j] as i64;
            for i in 0..FEATURE_COUNT {
                acc += ((input.get(i) as i64) * (self.w1[i * MLP_H1 + j] as i64)) >> 16;
            }
            h1[j] = leaky_relu(acc.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
        }

        // Layer 2 : 128 → 64
        let mut h2 = [0i32; MLP_H2];
        for j in 0..MLP_H2 {
            let mut acc: i64 = self.b2[j] as i64;
            for i in 0..MLP_H1 {
                acc += ((h1[i] as i64) * (self.w2[i * MLP_H2 + j] as i64)) >> 16;
            }
            h2[j] = leaky_relu(acc.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
        }

        // Layer 3 : 64 → 4, Sigmoid
        let mut out = [0i32; MLP_OUT];
        for j in 0..MLP_OUT {
            let mut acc: i64 = self.b3[j] as i64;
            for i in 0..MLP_H2 {
                acc += ((h2[i] as i64) * (self.w3[i * MLP_OUT + j] as i64)) >> 16;
            }
            out[j] = sigmoid(acc.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
        }
        out
    }

    /// P(Malicious) = out[2], en Q16.16 [0, 65536].
    #[inline]
    pub fn malicious_prob(out: &[i32; MLP_OUT]) -> i32 {
        out[OUT_MALICIOUS]
    }

    /// Classification basée sur les sorties et le seuil.
    pub fn classify_output(out: &[i32; MLP_OUT], threshold: i32) -> Classification {
        let p_mal = out[OUT_MALICIOUS];
        let p_sus = out[1];
        let p_ben = out[0];
        let p_unk = out[3];
        if p_mal >= threshold {
            Classification::Malicious
        } else if p_sus >= threshold / 2 || p_mal >= threshold / 2 {
            Classification::Suspicious
        } else if p_ben > p_unk {
            Classification::Benign
        } else {
            Classification::Unknown
        }
    }

    /// Convertit P(Malicious) Q16.16 en score 0..1000.
    #[inline]
    pub fn prob_to_score(prob_q16: i32) -> u32 {
        ((prob_q16.max(0) as u64 * 1_000) >> 16) as u32
    }
}

// ── Static BSS ────────────────────────────────────────────────────────────────

static MLP_MODEL: Mutex<MlpWeights> = Mutex::new(MlpWeights::zero());
static MLP_READY: AtomicBool = AtomicBool::new(false);

/// Initialise le MLP global avec des poids domain-informed.
pub fn mlp_init(seed: u32) {
    MLP_MODEL.lock().init_domain_seeded(seed);
    MLP_READY.store(true, Ordering::Release);
}

/// Inférence sur un feature vector. Retourne (P(Malicious) Q16.16, Classification).
pub fn mlp_infer(fv: &FeatureVector) -> (i32, Classification) {
    let model = MLP_MODEL.lock();
    let out = model.forward(fv);
    let prob = MlpWeights::malicious_prob(&out);
    let cls = MlpWeights::classify_output(&out, MLP_DEFAULT_THRESHOLD);
    (prob, cls)
}

/// Remplace les poids courants (ex : depuis une mise à jour entraînée).
pub fn mlp_update_weights(w: &MlpWeights) {
    let mut model = MLP_MODEL.lock();
    model.w1.copy_from_slice(&w.w1);
    model.b1.copy_from_slice(&w.b1);
    model.w2.copy_from_slice(&w.w2);
    model.b2.copy_from_slice(&w.b2);
    model.w3.copy_from_slice(&w.w3);
    model.b3.copy_from_slice(&w.b3);
    model.version = w.version;
}

/// FIX-F3 : checksum FNV-1a 64-bit sur les poids aplatis.
/// DOIT matcher `checksum_mlp()` de tools/ml_training/train_ngav.py.
fn mlp_checksum_parts(
    w1: &[i32], b1: &[i32], w2: &[i32], b2: &[i32], w3: &[i32], b3: &[i32], version: u32,
) -> u64 {
    let mut h: u64 = 0x5151_5151_0000_0000;
    for arr in [w1, b1, w2, b2, w3, b3] {
        for &x in arr {
            h ^= (x as u32) as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3); // FNV-1a 64-bit prime
        }
    }
    h ^ (version as u64)
}

/// FIX-F3/F4 : charge les poids ENTRAÎNÉS (`trained_weights.rs`) via un chemin
/// AUTHENTIFIÉ — vérifie le checksum d'intégrité ET la monotonie de version.
/// Retourne `false` (en gardant les poids seedés) si la vérification échoue :
/// pas d'écrasement silencieux possible du modèle phare du NGAV.
pub fn mlp_load_trained() -> bool {
    use super::trained_weights as tw;
    let cks = mlp_checksum_parts(
        &tw::MLP_W1, &tw::MLP_B1, &tw::MLP_W2, &tw::MLP_B2, &tw::MLP_W3, &tw::MLP_B3,
        tw::TRAINED_MLP_VERSION,
    );
    if cks != tw::TRAINED_MLP_CHECKSUM {
        return false; // intégrité KO
    }
    let mut model = MLP_MODEL.lock();
    if tw::TRAINED_MLP_VERSION <= model.version {
        return false; // anti-régression de version
    }
    model.w1.copy_from_slice(&tw::MLP_W1);
    model.b1.copy_from_slice(&tw::MLP_B1);
    model.w2.copy_from_slice(&tw::MLP_W2);
    model.b2.copy_from_slice(&tw::MLP_B2);
    model.w3.copy_from_slice(&tw::MLP_W3);
    model.b3.copy_from_slice(&tw::MLP_B3);
    model.version = tw::TRAINED_MLP_VERSION;
    MLP_READY.store(true, Ordering::Release);
    true
}

pub fn mlp_version() -> u32 {
    MLP_MODEL.lock().version
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model(seed: u32) -> MlpWeights {
        let mut m = MlpWeights::zero();
        m.init_domain_seeded(seed);
        m
    }

    #[test]
    fn mlp_forward_shape_bounded() {
        let m = make_model(42);
        let fv = FeatureVector::from_raw([100i32; FEATURE_COUNT]);
        let out = m.forward(&fv);
        assert_eq!(out.len(), MLP_OUT);
        for i in 0..MLP_OUT {
            assert!(out[i] >= 0 && out[i] <= 1 << 16, "out[{}]={}", i, out[i]);
        }
    }

    #[test]
    fn mlp_sigmoid_bounded_on_extreme_input() {
        let m = make_model(7);
        let fv = FeatureVector::from_raw([i32::MAX / 1000; FEATURE_COUNT]);
        let out = m.forward(&fv);
        for i in 0..MLP_OUT {
            assert!(out[i] >= 0 && out[i] <= 1 << 16, "out[{}]={}", i, out[i]);
        }
    }

    #[test]
    fn mlp_neg_extreme_bounded() {
        let m = make_model(3);
        let fv = FeatureVector::from_raw([i32::MIN / 1000; FEATURE_COUNT]);
        let out = m.forward(&fv);
        for i in 0..MLP_OUT {
            assert!(out[i] >= 0 && out[i] <= 1 << 16, "out[{}]={}", i, out[i]);
        }
    }

    #[test]
    fn mlp_prob_to_score_bounds() {
        assert_eq!(MlpWeights::prob_to_score(0), 0);
        assert_eq!(MlpWeights::prob_to_score(1 << 16), 1000);
        let mid = MlpWeights::prob_to_score(1 << 15);
        assert!(mid >= 499 && mid <= 501, "mid={}", mid);
    }

    #[test]
    fn mlp_classify_malicious_threshold() {
        // Au-dessus du seuil → Malicious
        let out = [0i32, 0, 40_000, 0];
        assert_eq!(
            MlpWeights::classify_output(&out, MLP_DEFAULT_THRESHOLD),
            Classification::Malicious
        );
        // Mi-chemin → Suspicious
        let out2 = [0i32, 0, 20_000, 0];
        assert_eq!(
            MlpWeights::classify_output(&out2, MLP_DEFAULT_THRESHOLD),
            Classification::Suspicious
        );
        // Faible, Benign gagne
        let out3 = [50_000i32, 0, 1_000, 0];
        assert_eq!(
            MlpWeights::classify_output(&out3, MLP_DEFAULT_THRESHOLD),
            Classification::Benign
        );
    }

    #[test]
    fn mlp_different_seeds_different_output() {
        let m1 = make_model(1);
        let m2 = make_model(2);
        let fv = FeatureVector::from_raw([50i32; FEATURE_COUNT]);
        let o1 = m1.forward(&fv);
        let o2 = m2.forward(&fv);
        assert!(o1.iter().zip(o2.iter()).any(|(a, b)| a != b));
    }

    #[test]
    fn mlp_zero_input_bounded() {
        let m = make_model(99);
        let fv = FeatureVector::zero();
        let out = m.forward(&fv);
        for i in 0..MLP_OUT {
            assert!(out[i] >= 0 && out[i] <= 1 << 16);
        }
    }
}
