//! # Anomaly Detection — Détection d'anomalies avec baselines et scores
//!
//! Système de détection d'anomalies basé sur le suivi de baselines
//! statistiques et le calcul de scores de déviation.
//!
//! ## Fonctionnalités
//! - Baseline par métrique (moyenne mobile, variance, compteur d'échantillons)
//! - Seuils d'anomalie configurables par métrique
//! - Génération d'alertes avec niveau de sévérité
//! - Détection en temps réel avec mise à jour incrémentale
//!
//! ## Règles
//! - NS-01 : uniquement core::sync::atomic + spin, pas de heap
//! - Zéro stub, zéro TODO, zéro placeholder

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Nombre maximum de métriques surveillées.
pub const MAX_METRICS: usize = 64;

/// Nombre maximum d'alertes en mémoire.
pub const MAX_ALERTS: usize = 32;

/// Taille max du nom de métrique.
pub const METRIC_NAME_SIZE: usize = 16;

/// Valeur par défaut du seuil de déviation (en écarts-types).
pub const DEFAULT_DEVIATION_THRESHOLD: u32 = 30; // ×0.1 → 3.0 σ

/// Facteur de lissage pour la moyenne mobile exponentielle (×256).
const EMA_ALPHA: u64 = 26; // ≈ 0.1

/// Valeur maximale représentable pour le score de déviation.
pub const MAX_DEVIATION_SCORE: u32 = 1000;

// ── Niveau d'alerte ─────────────────────────────────────────────────────────

/// Niveau de sévérité d'une alerte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub enum AlertLevel {
    Info = 0,
    Warning = 1,
    Critical = 2,
    Emergency = 3,
}

impl AlertLevel {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(AlertLevel::Info),
            1 => Some(AlertLevel::Warning),
            2 => Some(AlertLevel::Critical),
            3 => Some(AlertLevel::Emergency),
            _ => None,
        }
    }
}

// ── Alerte ───────────────────────────────────────────────────────────────────

/// Alerte d'anomalie générée par le détecteur.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Alert {
    /// Identifiant de la métrique ayant déclenché l'alerte.
    pub metric_id: u32,
    /// Niveau de sévérité.
    pub level: AlertLevel,
    /// Score de déviation au moment de l'alerte (×0.1 σ).
    pub deviation_score: u32,
    /// Valeur observée.
    pub observed_value: u64,
    /// Valeur attendue (baseline mean).
    pub expected_value: u64,
    /// Horodatage TSC de l'alerte.
    pub timestamp_tsc: u64,
    /// PID du processus concerné (0 = système).
    pub pid: u32,
    /// L'alerte est-elle valide ?
    pub valid: bool,
    /// Réservé.
    _reserved: [u8; 3],
}

impl Alert {
    pub const fn empty() -> Self {
        Self {
            metric_id: 0,
            level: AlertLevel::Info,
            deviation_score: 0,
            observed_value: 0,
            expected_value: 0,
            timestamp_tsc: 0,
            pid: 0,
            valid: false,
            _reserved: [0; 3],
        }
    }
}

// ── Baseline ─────────────────────────────────────────────────────────────────

/// Baseline statistique pour une métrique.
/// Utilise la moyenne mobile exponentielle (EMA) pour la moyenne et la variance.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Baseline {
    /// Nom de la métrique.
    pub name: [u8; METRIC_NAME_SIZE],
    /// Moyenne mobile exponentielle (×256 pour précision fixe).
    pub ema_mean: u64,
    /// Variance mobile exponentielle (×256²).
    pub ema_variance: u64,
    /// Nombre d'échantillons observés.
    pub sample_count: u64,
    /// Valeur minimale observée.
    pub min_value: u64,
    /// Valeur maximale observée.
    pub max_value: u64,
    /// Somme cumulée (pour statistiques).
    pub sum: u64,
    /// La baseline est-elle active ?
    pub active: bool,
    /// Réservé.
    _reserved: [u8; 7],
}

impl Baseline {
    pub const fn empty() -> Self {
        Self {
            name: [0u8; METRIC_NAME_SIZE],
            ema_mean: 0,
            ema_variance: 0,
            sample_count: 0,
            min_value: u64::MAX,
            max_value: 0,
            sum: 0,
            active: false,
            _reserved: [0; 7],
        }
    }

    /// Crée une baseline avec un nom.
    pub fn new(name: &[u8]) -> Self {
        let mut name_buf = [0u8; METRIC_NAME_SIZE];
        let len = name.len().min(METRIC_NAME_SIZE - 1);
        name_buf[..len].copy_from_slice(&name[..len]);
        Self {
            name: name_buf,
            ..Baseline::empty()
        }
    }

    /// Met à jour la baseline avec une nouvelle observation.
    /// Utilise la formule EMA pour la moyenne et la variance.
    pub fn update(&mut self, value: u64) {
        let value_fixed = value * 256; // Point fixe ×256

        self.sample_count += 1;
        self.sum = self.sum.wrapping_add(value);

        if value < self.min_value {
            self.min_value = value;
        }
        if value > self.max_value {
            self.max_value = value;
        }

        if self.sample_count == 1 {
            // Premier échantillon
            self.ema_mean = value_fixed;
            self.ema_variance = 0;
        } else {
            // Mise à jour EMA
            let delta = if value_fixed >= self.ema_mean {
                value_fixed - self.ema_mean
            } else {
                self.ema_mean - value_fixed
            };

            // Nouvelle moyenne : ema_mean = ema_mean + alpha * (value - ema_mean)
            let adjustment = (delta * EMA_ALPHA) / 256;
            if value_fixed >= self.ema_mean {
                self.ema_mean = self.ema_mean.saturating_add(adjustment);
            } else {
                self.ema_mean = self.ema_mean.saturating_sub(adjustment);
            }

            // Nouvelle variance : ema_variance = (1-alpha) * ema_variance + alpha * delta^2
            // Pour éviter l'overflow, on utilise delta/256 au lieu de delta
            let delta_scaled = delta / 256;
            let delta_sq = delta_scaled.saturating_mul(delta_scaled);
            let var_adjustment = (delta_sq * EMA_ALPHA) / 256;
            let old_var_scaled = (self.ema_variance * (256 - EMA_ALPHA)) / 256;
            self.ema_variance = old_var_scaled.saturating_add(var_adjustment);
        }

        self.active = true;
    }

    /// Retourne la moyenne (arrondie à l'entier).
    pub fn mean(&self) -> u64 {
        self.ema_mean / 256
    }

    /// Retourne l'écart-type (arrondi à l'entier).
    /// La variance est en ×256², donc sqrt(variance)/256 = σ.
    pub fn std_dev(&self) -> u64 {
        if self.ema_variance == 0 {
            return 0;
        }
        // Approximation entière de la racine carrée (méthode de Newton)
        isqrt(self.ema_variance) / 256
    }

    /// Calcule le score de déviation pour une valeur observée.
    ///
    /// # Retour
    /// Score en ×0.1 σ (ex: 30 = 3.0 σ). 0 si pas assez de données.
    pub fn deviation_score(&self, value: u64) -> u32 {
        if self.sample_count < 3 {
            return 0; // Pas assez de données
        }

        let sigma = self.std_dev();
        if sigma == 0 {
            // Si σ = 0, toute déviation est significative
            let mean = self.mean();
            if value == mean {
                return 0;
            }
            let diff = if value > mean {
                value - mean
            } else {
                mean - value
            };
            return if diff > 0 { MAX_DEVIATION_SCORE } else { 0 };
        }

        let mean = self.mean();
        let diff = if value > mean {
            value - mean
        } else {
            mean - value
        };

        // Score = (diff / sigma) * 10
        let score = (diff * 10) / sigma;
        score.min(MAX_DEVIATION_SCORE as u64) as u32
    }
}

/// Racine carrée entière (méthode de Newton).
fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (n + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ── Seuil d'anomalie ─────────────────────────────────────────────────────────

/// Seuil d'anomalie configurable par métrique.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AnomalyThreshold {
    /// ID de la métrique.
    pub metric_id: u32,
    /// Seuil de déviation pour alerte Info (×0.1 σ).
    pub info_threshold: u32,
    /// Seuil de déviation pour alerte Warning (×0.1 σ).
    pub warning_threshold: u32,
    /// Seuil de déviation pour alerte Critical (×0.1 σ).
    pub critical_threshold: u32,
    /// Seuil de déviation pour alerte Emergency (×0.1 σ).
    pub emergency_threshold: u32,
    /// Le seuil est-il actif ?
    pub active: bool,
    /// Réservé.
    _reserved: [u8; 3],
}

impl AnomalyThreshold {
    pub const fn default_thresholds(metric_id: u32) -> Self {
        Self {
            metric_id,
            info_threshold: 20,      // 2.0 σ
            warning_threshold: 30,   // 3.0 σ
            critical_threshold: 50,  // 5.0 σ
            emergency_threshold: 80, // 8.0 σ
            active: true,
            _reserved: [0; 3],
        }
    }

    pub const fn empty() -> Self {
        Self {
            metric_id: 0,
            info_threshold: 0,
            warning_threshold: 0,
            critical_threshold: 0,
            emergency_threshold: 0,
            active: false,
            _reserved: [0; 3],
        }
    }

    /// Détermine le niveau d'alerte pour un score de déviation donné.
    pub fn classify(&self, deviation_score: u32) -> Option<AlertLevel> {
        if !self.active {
            return None;
        }
        if deviation_score >= self.emergency_threshold {
            Some(AlertLevel::Emergency)
        } else if deviation_score >= self.critical_threshold {
            Some(AlertLevel::Critical)
        } else if deviation_score >= self.warning_threshold {
            Some(AlertLevel::Warning)
        } else if deviation_score >= self.info_threshold {
            Some(AlertLevel::Info)
        } else {
            None
        }
    }
}

// ── Détecteur d'anomalies ────────────────────────────────────────────────────

static ANOMALY_DETECTOR: Mutex<AnomalyDetectorInner> = Mutex::new(AnomalyDetectorInner::new());

static TOTAL_OBSERVATIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_ANOMALIES: AtomicU64 = AtomicU64::new(0);
static TOTAL_ALERTS: AtomicU64 = AtomicU64::new(0);

struct AnomalyDetectorInner {
    baselines: [Baseline; MAX_METRICS],
    thresholds: [AnomalyThreshold; MAX_METRICS],
    alerts: [Alert; MAX_ALERTS],
    baseline_count: usize,
    alert_head: usize,  // Index de la prochaine écriture (circulaire)
    alert_count: usize, // Nombre d'alertes valides
}

impl AnomalyDetectorInner {
    const fn new() -> Self {
        Self {
            baselines: [Baseline::empty(); MAX_METRICS],
            thresholds: [AnomalyThreshold::empty(); MAX_METRICS],
            alerts: [Alert::empty(); MAX_ALERTS],
            baseline_count: 0,
            alert_head: 0,
            alert_count: 0,
        }
    }
}

// ── Lecture TSC ──────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── API publique ─────────────────────────────────────────────────────────────

/// Enregistre une nouvelle métrique pour le suivi.
///
/// # Retour
/// - L'ID de la métrique (0..MAX_METRICS-1) si succès, ou MAX_METRICS si plein.
pub fn register_metric(name: &[u8]) -> usize {
    let mut det = ANOMALY_DETECTOR.lock();

    if det.baseline_count >= MAX_METRICS {
        return MAX_METRICS;
    }

    let idx = det.baseline_count;
    det.baselines[idx] = Baseline::new(name);
    det.thresholds[idx] = AnomalyThreshold::default_thresholds(idx as u32);
    det.baseline_count += 1;

    idx
}

/// Enregistre une observation pour une métrique.
///
/// Met à jour la baseline et vérifie si une anomalie est détectée.
///
/// # Arguments
/// - `metric_id` : identifiant de la métrique (retourné par register_metric).
/// - `value` : valeur observée.
/// - `pid` : PID du processus concerné (0 = système).
///
/// # Retour
/// - Le niveau d'alerte si une anomalie est détectée, None sinon.
pub fn observe(metric_id: usize, value: u64, pid: u32) -> Option<AlertLevel> {
    let mut det = ANOMALY_DETECTOR.lock();

    if metric_id >= det.baseline_count {
        return None;
    }

    TOTAL_OBSERVATIONS.fetch_add(1, Ordering::Relaxed);

    // Calculer le score de déviation AVANT la mise à jour
    let deviation_score = det.baselines[metric_id].deviation_score(value);
    let expected = det.baselines[metric_id].mean();

    // Mettre à jour la baseline
    det.baselines[metric_id].update(value);

    // Vérifier le seuil
    let level = det.thresholds[metric_id].classify(deviation_score);

    if let Some(alert_level) = level {
        TOTAL_ANOMALIES.fetch_add(1, Ordering::Relaxed);

        // Créer une alerte
        let alert = Alert {
            metric_id: metric_id as u32,
            level: alert_level,
            deviation_score,
            observed_value: value,
            expected_value: expected,
            timestamp_tsc: read_tsc(),
            pid,
            valid: true,
            _reserved: [0; 3],
        };

        // Stocker l'alerte (buffer circulaire)
        let alert_idx = det.alert_head;
        det.alerts[alert_idx] = alert;
        det.alert_head = (alert_idx + 1) % MAX_ALERTS;
        if det.alert_count < MAX_ALERTS {
            det.alert_count += 1;
        }

        TOTAL_ALERTS.fetch_add(1, Ordering::Relaxed);
        Some(alert_level)
    } else {
        None
    }
}

/// Configure les seuils d'anomalie pour une métrique.
pub fn set_thresholds(
    metric_id: usize,
    info: u32,
    warning: u32,
    critical: u32,
    emergency: u32,
) -> bool {
    let mut det = ANOMALY_DETECTOR.lock();
    if metric_id >= det.baseline_count {
        return false;
    }
    det.thresholds[metric_id] = AnomalyThreshold {
        metric_id: metric_id as u32,
        info_threshold: info,
        warning_threshold: warning,
        critical_threshold: critical,
        emergency_threshold: emergency,
        active: true,
        _reserved: [0; 3],
    };
    true
}

/// Retourne la baseline pour une métrique.
pub fn get_baseline(metric_id: usize) -> Option<Baseline> {
    let det = ANOMALY_DETECTOR.lock();
    if metric_id >= det.baseline_count {
        return None;
    }
    Some(det.baselines[metric_id])
}

/// Retourne les seuils pour une métrique.
pub fn get_thresholds(metric_id: usize) -> Option<AnomalyThreshold> {
    let det = ANOMALY_DETECTOR.lock();
    if metric_id >= det.baseline_count {
        return None;
    }
    Some(det.thresholds[metric_id])
}

/// Récupère les alertes récentes.
///
/// # Arguments
/// - `buffer` : tableau de destination.
/// - `max_count` : nombre maximum d'alertes à récupérer.
///
/// # Retour
/// Le nombre d'alertes copiées.
pub fn get_recent_alerts(buffer: &mut [Alert], max_count: usize) -> usize {
    let det = ANOMALY_DETECTOR.lock();
    let limit = max_count.min(buffer.len()).min(det.alert_count);
    let mut copied = 0usize;

    // Les alertes les plus récentes sont avant alert_head dans le buffer circulaire
    for i in 0..limit {
        // Calculer l'index dans le buffer circulaire (du plus récent au plus ancien)
        let idx = if det.alert_head >= i + 1 {
            det.alert_head - i - 1
        } else {
            MAX_ALERTS - (i + 1 - det.alert_head)
        };
        if det.alerts[idx].valid {
            buffer[copied] = det.alerts[idx];
            copied += 1;
            if copied >= buffer.len() || copied >= max_count {
                break;
            }
        }
    }

    copied
}

/// Calcule le score de déviation pour une valeur hypothétique.
///
/// Ne modifie pas la baseline. Utile pour l'analyse prédictive.
pub fn compute_deviation(metric_id: usize, value: u64) -> u32 {
    let det = ANOMALY_DETECTOR.lock();
    if metric_id >= det.baseline_count {
        return 0;
    }
    det.baselines[metric_id].deviation_score(value)
}

/// Réinitialise la baseline d'une métrique (oubli complet).
pub fn reset_baseline(metric_id: usize) -> bool {
    let mut det = ANOMALY_DETECTOR.lock();
    if metric_id >= det.baseline_count {
        return false;
    }
    let name = det.baselines[metric_id].name;
    det.baselines[metric_id] = Baseline {
        name,
        ..Baseline::empty()
    };
    true
}

/// Statistiques du détecteur d'anomalies.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AnomalyStats {
    pub metric_count: usize,
    pub total_observations: u64,
    pub total_anomalies: u64,
    pub total_alerts: u64,
    pub active_alerts: usize,
}

/// Retourne les statistiques du détecteur.
pub fn get_anomaly_stats() -> AnomalyStats {
    let det = ANOMALY_DETECTOR.lock();
    AnomalyStats {
        metric_count: det.baseline_count,
        total_observations: TOTAL_OBSERVATIONS.load(Ordering::Relaxed),
        total_anomalies: TOTAL_ANOMALIES.load(Ordering::Relaxed),
        total_alerts: TOTAL_ALERTS.load(Ordering::Relaxed),
        active_alerts: det.alert_count,
    }
}

/// Initialise le détecteur d'anomalies.
pub fn anomaly_init() {
    let mut det = ANOMALY_DETECTOR.lock();
    for i in 0..MAX_METRICS {
        det.baselines[i] = Baseline::empty();
        det.thresholds[i] = AnomalyThreshold::empty();
    }
    for i in 0..MAX_ALERTS {
        det.alerts[i] = Alert::empty();
    }
    det.baseline_count = 0;
    det.alert_head = 0;
    det.alert_count = 0;

    TOTAL_OBSERVATIONS.store(0, Ordering::Release);
    TOTAL_ANOMALIES.store(0, Ordering::Release);
    TOTAL_ALERTS.store(0, Ordering::Release);
}
