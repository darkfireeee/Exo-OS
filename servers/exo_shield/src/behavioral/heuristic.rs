//! # Heuristic Analysis — Analyse heuristique avec règles de détection
//!
//! Système d'analyse heuristique basé sur des règles de détection
//! avec un système de scoring pondéré. Chaque règle contribue à un
//! score de menace global par processus.
//!
//! ## Fonctionnalités
//! - Règles heuristiques configurables (max 64)
//! - Détection par patterns de comportement
//! - Scoring pondéré par sévérité et confiance
//! - Seuils d'alerte configurables
//!
//! ## Règles
//! - NS-01 : uniquement core::sync::atomic + spin, pas de heap
//! - Zéro stub, zéro TODO, zéro placeholder

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Nombre maximum de règles heuristiques.
pub const MAX_HEURISTIC_RULES: usize = 64;

/// Nombre maximum de scores de processus.
pub const MAX_PROCESS_SCORES: usize = 32;

/// Score maximum (plafond).
pub const MAX_HEURISTIC_SCORE: u32 = 1000;

/// Taille max du nom de règle.
pub const RULE_NAME_SIZE: usize = 24;

/// Taille max du pattern de comportement.
pub const BEHAVIOR_PATTERN_SIZE: usize = 16;

// ── Type de comportement ─────────────────────────────────────────────────────

/// Type de comportement détectable par heuristique.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum BehaviorType {
    /// Appel système inhabituel.
    SyscallAnomaly    = 0,
    /// Accès mémoire suspect.
    MemoryAnomaly     = 1,
    /// Activité réseau anormale.
    NetworkAnomaly    = 2,
    /// Schéma IPC suspect.
    IpcAnomaly        = 3,
    /// Accès fichier suspect.
    FileAccessAnomaly = 4,
    /// escalation de privilèges.
    PrivilegeEscalation = 5,
    /// Exécution de code suspect.
    CodeExecution     = 6,
    /// Persistance (auto-start, etc.).
    Persistence       = 7,
    /// Fuite de données.
    DataExfiltration  = 8,
    /// Comportement personnalisé.
    Custom            = 9,
}

impl BehaviorType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(BehaviorType::SyscallAnomaly),
            1 => Some(BehaviorType::MemoryAnomaly),
            2 => Some(BehaviorType::NetworkAnomaly),
            3 => Some(BehaviorType::IpcAnomaly),
            4 => Some(BehaviorType::FileAccessAnomaly),
            5 => Some(BehaviorType::PrivilegeEscalation),
            6 => Some(BehaviorType::CodeExecution),
            7 => Some(BehaviorType::Persistence),
            8 => Some(BehaviorType::DataExfiltration),
            9 => Some(BehaviorType::Custom),
            _ => None,
        }
    }
}

// ── Opérateur de comparaison ─────────────────────────────────────────────────

/// Opérateur de comparaison pour les conditions heuristiques.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum CompareOp {
    Eq      = 0,  // ==
    Ne      = 1,  // !=
    Gt      = 2,  // >
    Ge      = 3,  // >=
    Lt      = 4,  // <
    Le      = 5,  // <=
    Between = 6,  // min <= x <= max
}

impl CompareOp {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(CompareOp::Eq),
            1 => Some(CompareOp::Ne),
            2 => Some(CompareOp::Gt),
            3 => Some(CompareOp::Ge),
            4 => Some(CompareOp::Lt),
            5 => Some(CompareOp::Le),
            6 => Some(CompareOp::Between),
            _ => None,
        }
    }

    /// Évalue la comparaison.
    pub fn evaluate(&self, value: u64, threshold: u64, upper: u64) -> bool {
        match self {
            CompareOp::Eq => value == threshold,
            CompareOp::Ne => value != threshold,
            CompareOp::Gt => value > threshold,
            CompareOp::Ge => value >= threshold,
            CompareOp::Lt => value < threshold,
            CompareOp::Le => value <= threshold,
            CompareOp::Between => value >= threshold && value <= upper,
        }
    }
}

// ── Règle heuristique ────────────────────────────────────────────────────────

/// Une règle heuristique de détection.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HeuristicRule {
    /// Nom de la règle.
    pub name: [u8; RULE_NAME_SIZE],
    /// Type de comportement.
    pub behavior_type: BehaviorType,
    /// Opérateur de comparaison.
    pub op: CompareOp,
    /// Métrique à tester (index dans le profil du processus).
    pub metric_index: u8,
    /// Seuil de déclenchement.
    pub threshold: u64,
    /// Borne supérieure (pour Between).
    pub upper_bound: u64,
    /// Poids de la règle dans le score (1..=100).
    pub weight: u8,
    /// Confiance de la règle (1..=100, 100 = certitude absolue).
    pub confidence: u8,
    /// Score ajouté si la règle se déclenche (1..=100).
    pub score_contribution: u8,
    /// Pattern de comportement (octets à matcher).
    pub pattern: [u8; BEHAVIOR_PATTERN_SIZE],
    /// Longueur du pattern.
    pub pattern_len: u8,
    /// La règle est-elle active ?
    pub enabled: bool,
    /// Réservé.
    _reserved: [u8; 2],
}

impl HeuristicRule {
    pub const fn empty() -> Self {
        Self {
            name: [0u8; RULE_NAME_SIZE],
            behavior_type: BehaviorType::Custom,
            op: CompareOp::Eq,
            metric_index: 0,
            threshold: 0,
            upper_bound: 0,
            weight: 0,
            confidence: 0,
            score_contribution: 0,
            pattern: [0u8; BEHAVIOR_PATTERN_SIZE],
            pattern_len: 0,
            enabled: false,
            _reserved: [0; 2],
        }
    }

    /// Crée une nouvelle règle heuristique.
    pub fn new(
        name: &[u8],
        behavior_type: BehaviorType,
        op: CompareOp,
        metric_index: u8,
        threshold: u64,
        weight: u8,
        confidence: u8,
        score_contribution: u8,
    ) -> Self {
        let mut name_buf = [0u8; RULE_NAME_SIZE];
        let len = name.len().min(RULE_NAME_SIZE - 1);
        name_buf[..len].copy_from_slice(&name[..len]);
        Self {
            name: name_buf,
            behavior_type,
            op,
            metric_index,
            threshold,
            upper_bound: 0,
            weight: weight.min(100).max(1),
            confidence: confidence.min(100).max(1),
            score_contribution: score_contribution.min(100),
            pattern: [0u8; BEHAVIOR_PATTERN_SIZE],
            pattern_len: 0,
            enabled: true,
            _reserved: [0; 2],
        }
    }

    /// Définit le pattern de comportement.
    pub fn set_pattern(&mut self, pattern: &[u8]) {
        let len = pattern.len().min(BEHAVIOR_PATTERN_SIZE);
        self.pattern[..len].copy_from_slice(&pattern[..len]);
        self.pattern_len = len as u8;
    }

    /// Définit la borne supérieure (pour Between).
    pub fn set_upper_bound(&mut self, upper: u64) {
        self.upper_bound = upper;
        self.op = CompareOp::Between;
    }

    /// Évalue la règle contre une valeur de métrique.
    pub fn evaluate_metric(&self, value: u64) -> bool {
        if !self.enabled {
            return false;
        }
        self.op.evaluate(value, self.threshold, self.upper_bound)
    }

    /// Évalue la règle contre un buffer de données (pattern matching).
    pub fn evaluate_pattern(&self, data: &[u8]) -> bool {
        if !self.enabled || self.pattern_len == 0 {
            return false;
        }
        let plen = self.pattern_len as usize;
        if plen > data.len() {
            return false;
        }
        // Recherche de sous-séquence
        let search_limit = data.len() - plen + 1;
        for pos in 0..search_limit {
            let mut matched = true;
            for j in 0..plen {
                if data[pos + j] != self.pattern[j] {
                    matched = false;
                    break;
                }
            }
            if matched {
                return true;
            }
        }
        false
    }

    /// Calcule le score pondéré si la règle se déclenche.
    pub fn weighted_score(&self) -> u32 {
        let weight = self.weight as u32;
        let confidence = self.confidence as u32;
        let contribution = self.score_contribution as u32;
        // Score = contribution × (weight / 100) × (confidence / 100)
        (contribution * weight * confidence) / 10000
    }

    /// Retourne le nom comme slice.
    pub fn name_slice(&self) -> &[u8] {
        let mut len = 0usize;
        while len < RULE_NAME_SIZE && self.name[len] != 0 {
            len += 1;
        }
        &self.name[..len]
    }
}

// ── Score de processus ───────────────────────────────────────────────────────

/// Score de menace d'un processus, accumulé par les règles heuristiques.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ProcessScore {
    /// PID du processus.
    pub pid: u32,
    /// Score de menace cumulé.
    pub score: u32,
    /// Nombre de règles déclenchées.
    pub rules_triggered: u32,
    /// Horodatage du dernier déclenchement.
    pub last_trigger_tsc: u64,
    /// Types de comportement les plus fréquents (top 4).
    pub top_behaviors: [BehaviorType; 4],
    /// Compteur par type de comportement.
    pub behavior_counts: [u32; 10],
    /// Le score est-il valide ?
    pub valid: bool,
}

impl ProcessScore {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            score: 0,
            rules_triggered: 0,
            last_trigger_tsc: 0,
            top_behaviors: [BehaviorType::Custom; 4],
            behavior_counts: [0; 10],
            valid: false,
        }
    }
}

// ── Résultat d'analyse heuristique ───────────────────────────────────────────

/// Résultat de l'analyse heuristique pour un processus.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HeuristicResult {
    /// PID du processus.
    pub pid: u32,
    /// Score total.
    pub total_score: u32,
    /// Niveau de menace (0..=4).
    pub threat_level: u8,
    /// Nombre de règles déclenchées.
    pub triggered_count: u32,
    /// Liste des index de règles déclenchées (max 16).
    pub triggered_rules: [usize; 16],
    /// Horodatage.
    pub timestamp_tsc: u64,
}

impl HeuristicResult {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            total_score: 0,
            threat_level: 0,
            triggered_count: 0,
            triggered_rules: [0; 16],
            timestamp_tsc: 0,
        }
    }
}

// ── Moteur heuristique ───────────────────────────────────────────────────────

static HEURISTIC_ENGINE: Mutex<HeuristicEngineInner> = Mutex::new(HeuristicEngineInner::new());

static RULE_COUNT: AtomicU32 = AtomicU32::new(0);
static TOTAL_EVALUATIONS: AtomicU64 = AtomicU64::new(0);
static TOTAL_TRIGGERS: AtomicU64 = AtomicU64::new(0);

struct HeuristicEngineInner {
    rules: [HeuristicRule; MAX_HEURISTIC_RULES],
    rule_count: usize,
    process_scores: [ProcessScore; MAX_PROCESS_SCORES],
    score_count: usize,
}

impl HeuristicEngineInner {
    const fn new() -> Self {
        Self {
            rules: [HeuristicRule::empty(); MAX_HEURISTIC_RULES],
            rule_count: 0,
            process_scores: [ProcessScore::empty(); MAX_PROCESS_SCORES],
            score_count: 0,
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

/// Ajoute une règle heuristique au moteur.
///
/// # Retour
/// - L'index de la règle si succès, ou MAX_HEURISTIC_RULES si plein.
pub fn add_rule(rule: &HeuristicRule) -> usize {
    let mut engine = HEURISTIC_ENGINE.lock();

    // Chercher un slot libre
    for i in 0..engine.rule_count {
        if !engine.rules[i].enabled {
            engine.rules[i] = *rule;
            RULE_COUNT.fetch_add(1, Ordering::Relaxed);
            return i;
        }
    }

    if engine.rule_count >= MAX_HEURISTIC_RULES {
        return MAX_HEURISTIC_RULES;
    }

    let idx = engine.rule_count;
    engine.rules[idx] = *rule;
    engine.rule_count += 1;
    RULE_COUNT.fetch_add(1, Ordering::Relaxed);
    idx
}

/// Supprime une règle par index.
pub fn remove_rule(index: usize) -> bool {
    if index >= MAX_HEURISTIC_RULES {
        return false;
    }
    let mut engine = HEURISTIC_ENGINE.lock();
    if index >= engine.rule_count {
        return false;
    }
    if !engine.rules[index].enabled {
        return false;
    }
    engine.rules[index] = HeuristicRule::empty();
    RULE_COUNT.fetch_sub(1, Ordering::Relaxed);
    true
}

/// Active une règle par index.
pub fn enable_rule(index: usize) -> bool {
    let mut engine = HEURISTIC_ENGINE.lock();
    if index >= engine.rule_count {
        return false;
    }
    engine.rules[index].enabled = true;
    RULE_COUNT.fetch_add(1, Ordering::Relaxed);
    true
}

/// Désactive une règle par index.
pub fn disable_rule(index: usize) -> bool {
    let mut engine = HEURISTIC_ENGINE.lock();
    if index >= engine.rule_count {
        return false;
    }
    engine.rules[index].enabled = false;
    RULE_COUNT.fetch_sub(1, Ordering::Relaxed);
    true
}

/// Trouve ou crée un slot de score pour un PID.
fn find_or_create_score(engine: &mut HeuristicEngineInner, pid: u32) -> Option<usize> {
    // Chercher un slot existant
    for i in 0..engine.score_count {
        if engine.process_scores[i].valid && engine.process_scores[i].pid == pid {
            return Some(i);
        }
    }

    // Chercher un slot libre
    for i in 0..engine.score_count {
        if !engine.process_scores[i].valid {
            engine.process_scores[i] = ProcessScore {
                pid,
                valid: true,
                ..ProcessScore::empty()
            };
            return Some(i);
        }
    }

    // Créer un nouveau slot
    if engine.score_count < MAX_PROCESS_SCORES {
        let idx = engine.score_count;
        engine.process_scores[idx] = ProcessScore {
            pid,
            valid: true,
            ..ProcessScore::empty()
        };
        engine.score_count += 1;
        return Some(idx);
    }

    // Réutiliser le slot avec le score le plus bas
    let mut min_score = u32::MAX;
    let mut min_idx = 0;
    for i in 0..engine.score_count {
        if engine.process_scores[i].score < min_score {
            min_score = engine.process_scores[i].score;
            min_idx = i;
        }
    }
    engine.process_scores[min_idx] = ProcessScore {
        pid,
        valid: true,
        ..ProcessScore::empty()
    };
    Some(min_idx)
}

/// Évalue les règles heuristiques pour un processus.
///
/// # Arguments
/// - `pid` : PID du processus.
/// - `metrics` : valeurs des métriques du processus (indexées par metric_index).
/// - `data` : données brutes pour le pattern matching.
///
/// # Retour
/// Un `HeuristicResult` avec les règles déclenchées et le score.
pub fn evaluate(pid: u32, metrics: &[u64], data: &[u8]) -> HeuristicResult {
    let mut engine = HEURISTIC_ENGINE.lock();
    TOTAL_EVALUATIONS.fetch_add(1, Ordering::Relaxed);

    let mut result = HeuristicResult::empty();
    result.pid = pid;
    result.timestamp_tsc = read_tsc();

    let score_idx = find_or_create_score(&mut engine, pid);
    let mut total_score = 0u32;

    for i in 0..engine.rule_count {
        let rule = engine.rules[i];
        if !rule.enabled {
            continue;
        }

        let mut triggered = false;

        // Évaluation par métrique
        let metric_idx = rule.metric_index as usize;
        if metric_idx < metrics.len() {
            triggered = rule.evaluate_metric(metrics[metric_idx]);
        }

        // Évaluation par pattern (si pas déjà déclenché par métrique)
        if !triggered && rule.pattern_len > 0 && !data.is_empty() {
            triggered = rule.evaluate_pattern(data);
        }

        if triggered {
            let weighted = rule.weighted_score();
            total_score = total_score.saturating_add(weighted);

            if result.triggered_count < 16 {
                result.triggered_rules[result.triggered_count as usize] = i;
            }
            result.triggered_count += 1;

            // Mettre à jour le score du processus
            if let Some(sidx) = score_idx {
                let pscore = &mut engine.process_scores[sidx];
                pscore.score = pscore.score.saturating_add(weighted);
                pscore.rules_triggered += 1;
                pscore.last_trigger_tsc = read_tsc();

                // Compteur par type de comportement
                let bt_idx = rule.behavior_type as usize;
                if bt_idx < 10 {
                    pscore.behavior_counts[bt_idx] += 1;
                    let behavior_counts = pscore.behavior_counts;

                    // Mettre à jour le top 4 (tri par compteur)
                    update_top_behaviors(&mut pscore.top_behaviors, &behavior_counts);
                }
            }

            TOTAL_TRIGGERS.fetch_add(1, Ordering::Relaxed);
        }
    }

    // Plafonner le score
    total_score = total_score.min(MAX_HEURISTIC_SCORE);
    result.total_score = total_score;

    // Déterminer le niveau de menace
    result.threat_level = if total_score >= 800 {
        4 // Critique
    } else if total_score >= 500 {
        3 // Élevé
    } else if total_score >= 200 {
        2 // Moyen
    } else if total_score >= 50 {
        1 // Bas
    } else {
        0 // Négligeable
    };

    // Mettre à jour le score du processus avec le plafond
    if let Some(sidx) = score_idx {
        engine.process_scores[sidx].score = engine.process_scores[sidx].score.min(MAX_HEURISTIC_SCORE);
    }

    result
}

/// Met à jour le top 4 des types de comportement.
fn update_top_behaviors(top: &mut [BehaviorType; 4], counts: &[u32; 10]) {
    // Tri par sélection simple (4 éléments seulement)
    let mut indices = [0usize, 1, 2, 3, 4, 5, 6, 7, 8, 9];

    // Tri à bulles partiel (top 4)
    for i in 0..4 {
        for j in i + 1..10 {
            if counts[indices[j]] > counts[indices[i]] {
                let tmp = indices[i];
                indices[i] = indices[j];
                indices[j] = tmp;
            }
        }
    }

    for i in 0..4 {
        top[i] = BehaviorType::from_u8(indices[i] as u8).unwrap_or(BehaviorType::Custom);
    }
}

/// Évalue une seule règle contre une valeur.
///
/// # Retour
/// - Le score pondéré si déclenché, 0 sinon.
pub fn evaluate_single_rule(rule_index: usize, value: u64, data: &[u8]) -> u32 {
    let engine = HEURISTIC_ENGINE.lock();
    if rule_index >= engine.rule_count {
        return 0;
    }
    let rule = &engine.rules[rule_index];
    if !rule.enabled {
        return 0;
    }

    let mut triggered = rule.evaluate_metric(value);

    if !triggered && rule.pattern_len > 0 && !data.is_empty() {
        triggered = rule.evaluate_pattern(data);
    }

    if triggered {
        rule.weighted_score()
    } else {
        0
    }
}

/// Retourne le score d'un processus.
pub fn get_process_score(pid: u32) -> Option<ProcessScore> {
    let engine = HEURISTIC_ENGINE.lock();
    for i in 0..engine.score_count {
        if engine.process_scores[i].valid && engine.process_scores[i].pid == pid {
            return Some(engine.process_scores[i]);
        }
    }
    None
}

/// Réinitialise le score d'un processus.
pub fn reset_process_score(pid: u32) -> bool {
    let mut engine = HEURISTIC_ENGINE.lock();
    for i in 0..engine.score_count {
        if engine.process_scores[i].valid && engine.process_scores[i].pid == pid {
            engine.process_scores[i] = ProcessScore {
                pid,
                valid: true,
                ..ProcessScore::empty()
            };
            return true;
        }
    }
    false
}

/// Décrémente le score d'un processus (déclin exponentiel temporel).
///
/// Appelé périodiquement pour réduire les scores des processus
/// dont le comportement est redevenu normal.
pub fn decay_scores(decay_factor: u32) {
    let mut engine = HEURISTIC_ENGINE.lock();
    // decay_factor est en ×0.001 (ex: 990 = 0.99, réduit de 1%)
    for i in 0..engine.score_count {
        if !engine.process_scores[i].valid {
            continue;
        }
        let current = engine.process_scores[i].score;
        let new_score = (current as u64 * decay_factor as u64 / 1000) as u32;
        engine.process_scores[i].score = new_score;
        if new_score == 0 {
            engine.process_scores[i].rules_triggered = 0;
        }
    }
}

/// Statistiques du moteur heuristique.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HeuristicStats {
    pub active_rules: u32,
    pub tracked_processes: u32,
    pub total_evaluations: u64,
    pub total_triggers: u64,
}

/// Retourne les statistiques.
pub fn get_heuristic_stats() -> HeuristicStats {
    let engine = HEURISTIC_ENGINE.lock();
    let mut tracked = 0u32;
    for i in 0..engine.score_count {
        if engine.process_scores[i].valid {
            tracked += 1;
        }
    }
    HeuristicStats {
        active_rules: RULE_COUNT.load(Ordering::Relaxed),
        tracked_processes: tracked,
        total_evaluations: TOTAL_EVALUATIONS.load(Ordering::Relaxed),
        total_triggers: TOTAL_TRIGGERS.load(Ordering::Relaxed),
    }
}

/// Initialise le moteur heuristique.
pub fn heuristic_init() {
    let mut engine = HEURISTIC_ENGINE.lock();
    for i in 0..MAX_HEURISTIC_RULES {
        engine.rules[i] = HeuristicRule::empty();
    }
    for i in 0..MAX_PROCESS_SCORES {
        engine.process_scores[i] = ProcessScore::empty();
    }
    engine.rule_count = 0;
    engine.score_count = 0;

    RULE_COUNT.store(0, Ordering::Release);
    TOTAL_EVALUATIONS.store(0, Ordering::Release);
    TOTAL_TRIGGERS.store(0, Ordering::Release);
}
