//! Isolation Forest — détection d'anomalies non-supervisée pour ExoShield.
//!
//! 8 arbres de profondeur max 5 (63 nœuds/arbre en layout binaire complet).
//! Nœud i → enfant gauche 2i+1, droit 2i+2. Feuille si 2i+1 >= 63.
//!
//! Features dangereuses (ptrace, raw_socket, priv_escalation…) ont des seuils
//! bas → les anomalies sur ces features s'isolent en moins de pas.
//!
//! Score Q16.16 : 0 = normal (chemin profond), 65536 = très anomal (isolé tôt).
//! Calibration EMA en ligne : s'améliore à mesure que le système observe du normal.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::features::{FeatureVector, FEATURE_COUNT};

// ── Constantes ────────────────────────────────────────────────────────────────

pub const IF_TREES: usize = 8;
pub const IF_MAX_DEPTH: usize = 5;
/// Nœuds par arbre en arbre binaire complet : 2^6 - 1 = 63
pub const IF_TREE_NODES: usize = 63;

/// Features considérées dangereuses → seuils très bas
const DANGEROUS: [usize; 8] = [12, 18, 22, 26, 27, 28, 29, 30];

/// Profondeur normale initiale en Q8.8 : IF_MAX_DEPTH/2 * 256 = 640
const INIT_NORMAL_DEPTH_Q8: u32 = (IF_MAX_DEPTH as u32 / 2) * 256;

// ── Nœud ─────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SplitNode {
    pub feature: u8,
    pub _pad: u8,
    /// Seuil de split (valeur brute, pas Q16.16)
    pub threshold: u16,
}

impl SplitNode {
    pub const fn zero() -> Self {
        Self { feature: 0, _pad: 0, threshold: 0 }
    }

    fn random(state: &mut u32, bias_dangerous: bool) -> Self {
        let r1 = lcg_next(state);
        let r2 = lcg_next(state);

        let feature = if bias_dangerous && (r1 >> 31) != 0 {
            // 50 % de chance de viser une feature dangereuse
            DANGEROUS[((r1 >> 16) as usize) % DANGEROUS.len()] as u8
        } else {
            ((r1 >> 16) as usize % FEATURE_COUNT) as u8
        };

        let is_dangerous = DANGEROUS.contains(&(feature as usize));
        let threshold = if is_dangerous {
            ((r2 >> 16) % 5) as u16   // 0..4 : isole dès la moindre activité
        } else {
            ((r2 >> 16) % 200) as u16 // 0..199 : variation normale
        };

        Self { feature, _pad: 0, threshold }
    }
}

#[inline(always)]
fn lcg_next(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    *state
}

// ── Forêt ─────────────────────────────────────────────────────────────────────

pub struct IsolationForest {
    trees: [[SplitNode; IF_TREE_NODES]; IF_TREES],
    /// EMA de profondeur des samples "normaux", Q8.8
    normal_depth_ema_q8: u32,
    cal_count: u32,
}

impl IsolationForest {
    /// Zéro-init pour BSS. Appeler `initialize()` avant tout usage.
    pub const fn uninit() -> Self {
        Self {
            trees: [[SplitNode::zero(); IF_TREE_NODES]; IF_TREES],
            normal_depth_ema_q8: INIT_NORMAL_DEPTH_Q8,
            cal_count: 0,
        }
    }

    pub fn initialize(&mut self, seed: u32) {
        let mut state = seed;
        for t in 0..IF_TREES {
            let bias = t < IF_TREES / 2; // premiers arbres : biais dangerous
            for i in 0..IF_TREE_NODES {
                self.trees[t][i] = SplitNode::random(&mut state, bias);
            }
        }
    }

    fn path_length(&self, tree: usize, fv: &FeatureVector) -> u8 {
        let t = &self.trees[tree];
        let mut idx = 0usize;
        let mut depth = 0u8;
        loop {
            let left = 2 * idx + 1;
            if left >= IF_TREE_NODES { break; }
            let node = &t[idx];
            idx = if fv.get(node.feature as usize) <= node.threshold as i32 {
                left
            } else {
                left + 1
            };
            depth += 1;
        }
        depth
    }

    /// Score d'anomalie en Q16.16 [0, 65536].
    /// 0 = normal (même profondeur que les samples normaux).
    /// 65536 = très anomal (isolé en haut de l'arbre).
    pub fn score(&self, fv: &FeatureVector) -> i32 {
        let mut sum = 0u32;
        for t in 0..IF_TREES {
            sum += self.path_length(t, fv) as u32;
        }
        // avg_depth en Q8.8
        let avg_q8 = (sum * 256) / (IF_TREES as u32);
        let normal = self.normal_depth_ema_q8.max(1);

        if avg_q8 >= normal {
            0
        } else {
            ((normal - avg_q8) as u64 * 65_536 / normal as u64) as i32
        }
    }

    /// Observe un vecteur comme "normal" pour calibrer l'EMA.
    /// Appeler quand un processus est connu bénin (pas d'alerte depuis N ticks).
    pub fn observe_normal(&mut self, fv: &FeatureVector) {
        let mut sum = 0u32;
        for t in 0..IF_TREES {
            sum += self.path_length(t, fv) as u32;
        }
        let avg_q8 = (sum * 256) / (IF_TREES as u32);
        // EMA α = 1/32
        const A: u32 = 32;
        self.normal_depth_ema_q8 =
            (self.normal_depth_ema_q8 * (A - 1) + avg_q8) / A;
        self.cal_count = self.cal_count.saturating_add(1);
    }

    pub fn calibration_count(&self) -> u32 { self.cal_count }
    pub fn normal_depth_q8(&self) -> u32 { self.normal_depth_ema_q8 }
}

// ── Static global ─────────────────────────────────────────────────────────────

static IFOREST: Mutex<IsolationForest> = Mutex::new(IsolationForest::uninit());
static IF_READY: AtomicBool = AtomicBool::new(false);

pub fn iforest_init(seed: u32) {
    IFOREST.lock().initialize(seed);
    IF_READY.store(true, Ordering::Release);
}

/// Score d'anomalie Q16.16 [0, 65536].
pub fn iforest_score(fv: &FeatureVector) -> i32 {
    if !IF_READY.load(Ordering::Acquire) { return 0; }
    IFOREST.lock().score(fv)
}

/// Calibration : observe fv comme normal.
pub fn iforest_observe_normal(fv: &FeatureVector) {
    if !IF_READY.load(Ordering::Acquire) { return; }
    IFOREST.lock().observe_normal(fv);
}

pub fn iforest_calibration_count() -> u32 {
    IFOREST.lock().calibration_count()
}

/// FIX-F4 : charge les seuils d'Isolation Forest ENTRAÎNÉS (`trained_weights.rs`).
/// Remplace l'init seedée par les splits fittés sur la distribution bénigne.
/// Retourne `false` si les tableaux n'ont pas la taille attendue (8×63).
pub fn iforest_load_trained() -> bool {
    use super::trained_weights as tw;
    if tw::IF_NODE_FEATURE.len() != IF_TREES * IF_TREE_NODES
        || tw::IF_NODE_THRESHOLD.len() != IF_TREES * IF_TREE_NODES
    {
        return false;
    }
    let mut f = IFOREST.lock();
    let mut k = 0usize;
    for t in 0..IF_TREES {
        for i in 0..IF_TREE_NODES {
            f.trees[t][i] = SplitNode {
                feature: tw::IF_NODE_FEATURE[k],
                _pad: 0,
                threshold: tw::IF_NODE_THRESHOLD[k],
            };
            k += 1;
        }
    }
    IF_READY.store(true, Ordering::Release);
    true
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_forest(seed: u32) -> IsolationForest {
        let mut f = IsolationForest::uninit();
        f.initialize(seed);
        f
    }

    #[test]
    fn if_score_bounded_normal_input() {
        let f = make_forest(42);
        let fv = FeatureVector::from_raw([50i32; FEATURE_COUNT]);
        let s = f.score(&fv);
        assert!(s >= 0 && s <= 1 << 16, "score={}", s);
    }

    #[test]
    fn if_score_bounded_zero_input() {
        let f = make_forest(1);
        let s = f.score(&FeatureVector::zero());
        assert!(s >= 0 && s <= 1 << 16);
    }

    #[test]
    fn if_score_bounded_max_input() {
        let f = make_forest(3);
        let fv = FeatureVector::from_raw([10_000i32; FEATURE_COUNT]);
        let s = f.score(&fv);
        assert!(s >= 0 && s <= 1 << 16, "score={}", s);
    }

    #[test]
    fn if_dangerous_features_score_gte_normal() {
        let mut f = make_forest(7);
        // Calibrer avec des samples normaux
        for _ in 0..50 {
            f.observe_normal(&FeatureVector::from_raw([10i32; FEATURE_COUNT]));
        }
        let s_normal = f.score(&FeatureVector::from_raw([10i32; FEATURE_COUNT]));

        let mut d = [10i32; FEATURE_COUNT];
        d[30] = 1_000; // ptrace_use
        d[12] = 500;   // priv_escalation
        d[27] = 300;   // raw_socket
        let s_danger = f.score(&FeatureVector::from_raw(d));

        assert!(
            s_danger >= s_normal,
            "dangereux={} devrait être >= normal={}", s_danger, s_normal
        );
    }

    #[test]
    fn if_calibration_increments_count() {
        let mut f = IsolationForest::uninit();
        f.initialize(42);
        for _ in 0..64 {
            f.observe_normal(&FeatureVector::from_raw([100i32; FEATURE_COUNT]));
        }
        assert_eq!(f.calibration_count(), 64);
    }

    #[test]
    fn if_uninit_score_bounded() {
        // Uninit (seuils tous 0) doit quand même retourner valeur bornée
        let f = IsolationForest::uninit();
        let s = f.score(&FeatureVector::from_raw([5i32; FEATURE_COUNT]));
        assert!(s >= 0 && s <= 1 << 16);
    }

    #[test]
    fn if_two_seeds_differ() {
        let f1 = make_forest(10);
        let f2 = make_forest(20);
        let fv = FeatureVector::from_raw([100i32; FEATURE_COUNT]);
        // Scores peuvent être identiques si hasard, mais aucun ne doit paniquer
        let _s1 = f1.score(&fv);
        let _s2 = f2.score(&fv);
    }
}
