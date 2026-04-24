//! # YARA-like Rule Engine — Évaluation de règles de détection
//!
//! Moteur de règles inspiré de YARA avec conditions simples :
//! equals, contains, greater_than. Évaluation sur des buffers de données
//! avec actions associées (alert, block, log).
//!
//! ## Règles
//! - NS-01 : uniquement core::sync::atomic + spin, pas de heap
//! - Zéro stub, zéro TODO, zéro placeholder

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Nombre maximum de règles dans le moteur.
pub const MAX_RULES: usize = 128;

/// Taille max du nom de règle.
pub const RULE_NAME_SIZE: usize = 32;

/// Nombre maximum de conditions par règle.
pub const MAX_CONDITIONS_PER_RULE: usize = 8;

/// Nombre maximum de résultats d'évaluation.
pub const MAX_EVAL_RESULTS: usize = 16;

// ── Type de condition ────────────────────────────────────────────────────────

/// Type de condition dans une règle YARA-like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum ConditionType {
    /// Égalité exacte (field[offset..offset+len] == value).
    Equals = 0,
    /// Contient (field contient value comme sous-séquence).
    Contains = 1,
    /// Supérieur à (valeur numérique en little-endian > threshold).
    GreaterThan = 2,
    /// Inférieur à (valeur numérique en little-endian < threshold).
    LessThan = 3,
    /// Non égal.
    NotEquals = 4,
    /// Correspondance de bits (AND bit-à-bit).
    BitwiseAnd = 5,
}

impl ConditionType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(ConditionType::Equals),
            1 => Some(ConditionType::Contains),
            2 => Some(ConditionType::GreaterThan),
            3 => Some(ConditionType::LessThan),
            4 => Some(ConditionType::NotEquals),
            5 => Some(ConditionType::BitwiseAnd),
            _ => None,
        }
    }
}

// ── Champ de données ─────────────────────────────────────────────────────────

/// Champ de données sur lequel une condition s'applique.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum FieldType {
    /// Buffer de données brut.
    RawData = 0,
    /// En-tête réseau.
    NetHeader = 1,
    /// Charge utile (payload).
    Payload = 2,
    /// Métadonnées de fichier.
    FileMeta = 3,
    /// Registre mémoire.
    Memory = 4,
}

impl FieldType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(FieldType::RawData),
            1 => Some(FieldType::NetHeader),
            2 => Some(FieldType::Payload),
            3 => Some(FieldType::FileMeta),
            4 => Some(FieldType::Memory),
            _ => None,
        }
    }
}

// ── Condition ────────────────────────────────────────────────────────────────

/// Une condition dans une règle YARA-like.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Condition {
    /// Type de condition.
    pub cond_type: ConditionType,
    /// Champ de données à tester.
    pub field: FieldType,
    /// Décalage dans le champ (en octets).
    pub offset: u16,
    /// Longueur de la valeur à comparer (en octets, max 8).
    pub length: u8,
    /// Valeur de comparaison (jusqu'à 8 octets en little-endian).
    pub value: [u8; 8],
    /// Seuil numérique pour GreaterThan/LessThan (little-endian u64).
    pub threshold: u64,
    /// Opérateur logique avec la condition suivante (AND/OR).
    pub logic_op: LogicOp,
    /// La condition est-elle active ?
    pub enabled: bool,
    /// Réservé.
    _reserved: [u8; 3],
}

impl Condition {
    pub const fn empty() -> Self {
        Self {
            cond_type: ConditionType::Equals,
            field: FieldType::RawData,
            offset: 0,
            length: 0,
            value: [0u8; 8],
            threshold: 0,
            logic_op: LogicOp::And,
            enabled: false,
            _reserved: [0; 3],
        }
    }

    /// Crée une condition d'égalité.
    pub fn equals(field: FieldType, offset: u16, value: &[u8]) -> Self {
        let len = value.len().min(8);
        let mut val = [0u8; 8];
        val[..len].copy_from_slice(&value[..len]);
        Self {
            cond_type: ConditionType::Equals,
            field,
            offset,
            length: len as u8,
            value: val,
            threshold: 0,
            logic_op: LogicOp::And,
            enabled: true,
            _reserved: [0; 3],
        }
    }

    /// Crée une condition "contient".
    pub fn contains(field: FieldType, value: &[u8]) -> Self {
        let len = value.len().min(8);
        let mut val = [0u8; 8];
        val[..len].copy_from_slice(&value[..len]);
        Self {
            cond_type: ConditionType::Contains,
            field,
            offset: 0,
            length: len as u8,
            value: val,
            threshold: 0,
            logic_op: LogicOp::And,
            enabled: true,
            _reserved: [0; 3],
        }
    }

    /// Crée une condition "supérieur à".
    pub fn greater_than(field: FieldType, offset: u16, threshold: u64) -> Self {
        Self {
            cond_type: ConditionType::GreaterThan,
            field,
            offset,
            length: 8,
            value: threshold.to_le_bytes(),
            threshold,
            logic_op: LogicOp::And,
            enabled: true,
            _reserved: [0; 3],
        }
    }

    /// Crée une condition "inférieur à".
    pub fn less_than(field: FieldType, offset: u16, threshold: u64) -> Self {
        Self {
            cond_type: ConditionType::LessThan,
            field,
            offset,
            length: 8,
            value: threshold.to_le_bytes(),
            threshold,
            logic_op: LogicOp::And,
            enabled: true,
            _reserved: [0; 3],
        }
    }

    /// Crée une condition "non égal".
    pub fn not_equals(field: FieldType, offset: u16, value: &[u8]) -> Self {
        let len = value.len().min(8);
        let mut val = [0u8; 8];
        val[..len].copy_from_slice(&value[..len]);
        Self {
            cond_type: ConditionType::NotEquals,
            field,
            offset,
            length: len as u8,
            value: val,
            threshold: 0,
            logic_op: LogicOp::And,
            enabled: true,
            _reserved: [0; 3],
        }
    }

    /// Crée une condition "AND bit-à-bit".
    pub fn bitwise_and(field: FieldType, offset: u16, mask: u64) -> Self {
        Self {
            cond_type: ConditionType::BitwiseAnd,
            field,
            offset,
            length: 8,
            value: mask.to_le_bytes(),
            threshold: mask,
            logic_op: LogicOp::And,
            enabled: true,
            _reserved: [0; 3],
        }
    }
}

// ── Opérateur logique ────────────────────────────────────────────────────────

/// Opérateur logique entre conditions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum LogicOp {
    /// ET logique (défaut).
    And = 0,
    /// OU logique.
    Or = 1,
}

impl LogicOp {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(LogicOp::And),
            1 => Some(LogicOp::Or),
            _ => None,
        }
    }
}

// ── Action ───────────────────────────────────────────────────────────────────

/// Action à entreprendre si la règle correspond.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum RuleAction {
    /// Alerte uniquement (journalisation).
    Alert = 0,
    /// Blocage de l'opération.
    Block = 1,
    /// Journalisation silencieuse.
    Log = 2,
    /// Quarantaine.
    Quarantine = 3,
    /// Terminer le processus.
    Kill = 4,
}

impl RuleAction {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(RuleAction::Alert),
            1 => Some(RuleAction::Block),
            2 => Some(RuleAction::Log),
            3 => Some(RuleAction::Quarantine),
            4 => Some(RuleAction::Kill),
            _ => None,
        }
    }
}

// ── Règle YARA-like ──────────────────────────────────────────────────────────

/// Une règle YARA-like avec nom, conditions et action.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rule {
    /// Nom de la règle (chaîne C, terminée par \0).
    pub name: [u8; RULE_NAME_SIZE],
    /// Conditions de la règle.
    pub conditions: [Condition; MAX_CONDITIONS_PER_RULE],
    /// Nombre de conditions actives.
    pub condition_count: u8,
    /// Action à entreprendre si correspondance.
    pub action: RuleAction,
    /// La règle est-elle active ?
    pub enabled: bool,
    /// Priorité de la règle (0 = basse, 255 = haute).
    pub priority: u8,
    /// Sévérité (0..=3).
    pub severity: u8,
    /// Réservé.
    _reserved: [u8; 1],
}

impl Rule {
    pub const fn empty() -> Self {
        Self {
            name: [0u8; RULE_NAME_SIZE],
            conditions: [Condition::empty(); MAX_CONDITIONS_PER_RULE],
            condition_count: 0,
            action: RuleAction::Alert,
            enabled: false,
            priority: 0,
            severity: 0,
            _reserved: [0],
        }
    }

    /// Crée une nouvelle règle avec un nom.
    pub fn new(name: &[u8], action: RuleAction, priority: u8, severity: u8) -> Self {
        let mut name_buf = [0u8; RULE_NAME_SIZE];
        let len = name.len().min(RULE_NAME_SIZE - 1);
        name_buf[..len].copy_from_slice(&name[..len]);
        Self {
            name: name_buf,
            conditions: [Condition::empty(); MAX_CONDITIONS_PER_RULE],
            condition_count: 0,
            action,
            enabled: true,
            priority,
            severity: severity.min(3),
            _reserved: [0],
        }
    }

    /// Ajoute une condition à la règle.
    pub fn add_condition(&mut self, cond: Condition) -> bool {
        if self.condition_count as usize >= MAX_CONDITIONS_PER_RULE {
            return false;
        }
        self.conditions[self.condition_count as usize] = cond;
        self.condition_count += 1;
        true
    }

    /// Retourne le nom de la règle comme slice (sans le \0 final).
    pub fn name_slice(&self) -> &[u8] {
        let mut len = 0usize;
        while len < RULE_NAME_SIZE && self.name[len] != 0 {
            len += 1;
        }
        &self.name[..len]
    }
}

// ── Résultat d'évaluation ────────────────────────────────────────────────────

/// Résultat de l'évaluation d'une règle.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct EvalResult {
    /// Index de la règle dans le moteur.
    pub rule_index: usize,
    /// La règle a-t-elle correspondu ?
    pub matched: bool,
    /// Action recommandée.
    pub action: RuleAction,
    /// Priorité de la règle.
    pub priority: u8,
    /// Sévérité.
    pub severity: u8,
    /// Nombre de conditions évaluées.
    pub conditions_evaluated: u8,
    /// Nombre de conditions vraies.
    pub conditions_matched: u8,
}

impl EvalResult {
    pub const fn empty() -> Self {
        Self {
            rule_index: 0,
            matched: false,
            action: RuleAction::Alert,
            priority: 0,
            severity: 0,
            conditions_evaluated: 0,
            conditions_matched: 0,
        }
    }
}

/// Résultat complet de l'évaluation de toutes les règles.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RuleEvalResult {
    /// Nombre de résultats.
    pub count: usize,
    /// Résultats individuels.
    pub results: [EvalResult; MAX_EVAL_RESULTS],
    /// Nombre total de règles évaluées.
    pub rules_evaluated: usize,
    /// Nombre de règles correspondantes.
    pub rules_matched: usize,
    /// Score de menace global (0..=100).
    pub threat_score: u8,
}

impl RuleEvalResult {
    pub const fn empty() -> Self {
        Self {
            count: 0,
            results: [EvalResult::empty(); MAX_EVAL_RESULTS],
            rules_evaluated: 0,
            rules_matched: 0,
            threat_score: 0,
        }
    }
}

// ── Moteur de règles ─────────────────────────────────────────────────────────

/// Moteur de règles YARA-like avec stockage statique.
static RULE_ENGINE: Mutex<RuleEngineInner> = Mutex::new(RuleEngineInner::new());

static RULE_COUNT: AtomicU32 = AtomicU32::new(0);
static RULES_EVALUATED: AtomicU64 = AtomicU64::new(0);
static RULES_MATCHED: AtomicU64 = AtomicU64::new(0);

struct RuleEngineInner {
    rules: [Rule; MAX_RULES],
    count: usize,
}

impl RuleEngineInner {
    const fn new() -> Self {
        Self {
            rules: [Rule::empty(); MAX_RULES],
            count: 0,
        }
    }
}

// ── Évaluation de condition ──────────────────────────────────────────────────

/// Extrait une valeur numérique d'un buffer à un offset donné.
fn extract_u64(data: &[u8], offset: usize, len: u8) -> u64 {
    let l = len as usize;
    if l == 0 || offset + l > data.len() {
        return 0;
    }
    let mut buf = [0u8; 8];
    let copy_len = l.min(8);
    if offset + copy_len <= data.len() {
        buf[..copy_len].copy_from_slice(&data[offset..offset + copy_len]);
    }
    u64::from_le_bytes(buf)
}

/// Évalue une condition sur un buffer de données.
///
/// # Retour
/// - true si la condition est satisfaite, false sinon.
pub fn evaluate_condition(cond: &Condition, data: &[u8]) -> bool {
    if !cond.enabled {
        return false;
    }

    let offset = cond.offset as usize;
    let len = cond.length as usize;
    if len == 0 {
        return false;
    }

    match cond.cond_type {
        ConditionType::Equals => {
            if offset + len > data.len() {
                return false;
            }
            let mut matched = true;
            for i in 0..len {
                if data[offset + i] != cond.value[i] {
                    matched = false;
                    break;
                }
            }
            matched
        }

        ConditionType::NotEquals => {
            if offset + len > data.len() {
                return true; // Absence = non-égalité
            }
            let mut any_diff = false;
            for i in 0..len {
                if data[offset + i] != cond.value[i] {
                    any_diff = true;
                    break;
                }
            }
            any_diff
        }

        ConditionType::Contains => {
            if len > data.len() {
                return false;
            }
            // Recherche de sous-séquence
            let search_limit = data.len() - len + 1;
            let mut found = false;
            for pos in 0..search_limit {
                let mut match_ok = true;
                for j in 0..len {
                    if data[pos + j] != cond.value[j] {
                        match_ok = false;
                        break;
                    }
                }
                if match_ok {
                    found = true;
                    break;
                }
            }
            found
        }

        ConditionType::GreaterThan => {
            let val = extract_u64(data, offset, cond.length);
            val > cond.threshold
        }

        ConditionType::LessThan => {
            let val = extract_u64(data, offset, cond.length);
            val < cond.threshold
        }

        ConditionType::BitwiseAnd => {
            let val = extract_u64(data, offset, cond.length);
            (val & cond.threshold) != 0
        }
    }
}

/// Évalue toutes les conditions d'une règle avec les opérateurs logiques.
///
/// Les conditions sont évaluées séquentiellement. La première condition
/// n'a pas d'opérateur logique. Les conditions suivantes utilisent
/// leur `logic_op` pour se combiner avec le résultat précédent.
///
/// # Retour
/// - (résultat final, nombre de conditions évaluées, nombre de conditions vraies).
pub fn evaluate_rule(rule: &Rule, data: &[u8]) -> (bool, u8, u8) {
    if !rule.enabled || rule.condition_count == 0 {
        return (false, 0, 0);
    }

    let count = rule.condition_count as usize;
    let mut result = true; // Pour AND initial, true est neutre
    let mut first = true;
    let mut evaluated = 0u8;
    let mut matched = 0u8;

    for i in 0..count {
        let cond = &rule.conditions[i];
        if !cond.enabled {
            continue;
        }

        let cond_result = evaluate_condition(cond, data);
        evaluated += 1;
        if cond_result {
            matched += 1;
        }

        if first {
            result = cond_result;
            first = false;
        } else {
            match cond.logic_op {
                LogicOp::And => {
                    result = result && cond_result;
                }
                LogicOp::Or => {
                    result = result || cond_result;
                }
            }
        }
    }

    (result, evaluated, matched)
}

// ── Parseur de condition simple ──────────────────────────────────────────────

/// Parse une condition depuis un format binaire compact.
///
/// Format : [cond_type:u8, field:u8, offset:u16, length:u8, value:8, threshold:8, logic_op:u8]
/// Total : 29 octets.
///
/// # Retour
/// - La condition parsée, ou `Condition::empty()` si le format est invalide.
pub fn parse_condition(data: &[u8]) -> Condition {
    if data.len() < 29 {
        return Condition::empty();
    }

    let cond_type = match ConditionType::from_u8(data[0]) {
        Some(ct) => ct,
        None => return Condition::empty(),
    };

    let field = match FieldType::from_u8(data[1]) {
        Some(f) => f,
        None => return Condition::empty(),
    };

    let offset = u16::from_le_bytes([data[2], data[3]]);
    let length = data[4];

    let mut value = [0u8; 8];
    value.copy_from_slice(&data[5..13]);

    let threshold = u64::from_le_bytes([
        data[13], data[14], data[15], data[16], data[17], data[18], data[19], data[20],
    ]);

    let logic_op = match LogicOp::from_u8(data[21]) {
        Some(lo) => lo,
        None => LogicOp::And,
    };

    Condition {
        cond_type,
        field,
        offset,
        length,
        value,
        threshold,
        logic_op,
        enabled: true,
        _reserved: [0; 3],
    }
}

// ── API publique du moteur ───────────────────────────────────────────────────

/// Ajoute une règle au moteur.
///
/// # Retour
/// - L'index de la règle si succès, ou `usize::MAX` si le moteur est plein.
pub fn add_rule(rule: &Rule) -> usize {
    let mut engine = RULE_ENGINE.lock();

    // Chercher un slot libre
    for i in 0..engine.count {
        if !engine.rules[i].enabled {
            engine.rules[i] = *rule;
            RULE_COUNT.fetch_add(1, Ordering::Relaxed);
            return i;
        }
    }

    if engine.count >= MAX_RULES {
        return usize::MAX;
    }

    let idx = engine.count;
    engine.rules[idx] = *rule;
    engine.count += 1;
    RULE_COUNT.fetch_add(1, Ordering::Relaxed);
    idx
}

/// Supprime une règle par index.
pub fn remove_rule(index: usize) -> bool {
    if index >= MAX_RULES {
        return false;
    }
    let mut engine = RULE_ENGINE.lock();
    if index >= engine.count {
        return false;
    }
    if !engine.rules[index].enabled {
        return false;
    }
    engine.rules[index] = Rule::empty();
    RULE_COUNT.fetch_sub(1, Ordering::Relaxed);
    true
}

/// Active une règle par index.
pub fn enable_rule(index: usize) -> bool {
    if index >= MAX_RULES {
        return false;
    }
    let mut engine = RULE_ENGINE.lock();
    if index >= engine.count {
        return false;
    }
    engine.rules[index].enabled = true;
    true
}

/// Désactive une règle par index.
pub fn disable_rule(index: usize) -> bool {
    if index >= MAX_RULES {
        return false;
    }
    let mut engine = RULE_ENGINE.lock();
    if index >= engine.count {
        return false;
    }
    engine.rules[index].enabled = false;
    true
}

/// Évalue toutes les règles actives sur les données fournies.
///
/// # Retour
/// Un `RuleEvalResult` avec les résultats de l'évaluation.
pub fn evaluate_all(data: &[u8]) -> RuleEvalResult {
    let engine = RULE_ENGINE.lock();
    let mut result = RuleEvalResult::empty();

    for i in 0..engine.count {
        let rule = &engine.rules[i];
        if !rule.enabled {
            continue;
        }

        let (matched, eval_count, match_count) = evaluate_rule(rule, data);

        RULES_EVALUATED.fetch_add(1, Ordering::Relaxed);
        result.rules_evaluated += 1;

        if matched {
            RULES_MATCHED.fetch_add(1, Ordering::Relaxed);
            result.rules_matched += 1;

            if result.count < MAX_EVAL_RESULTS {
                result.results[result.count] = EvalResult {
                    rule_index: i,
                    matched: true,
                    action: rule.action,
                    priority: rule.priority,
                    severity: rule.severity,
                    conditions_evaluated: eval_count,
                    conditions_matched: match_count,
                };
                result.count += 1;
            }
        }
    }

    // Calculer le score de menace
    if result.rules_matched > 0 {
        let mut total_severity = 0u32;
        for i in 0..result.count {
            total_severity += result.results[i].severity as u32 + 1;
            total_severity += result.results[i].priority as u32 / 32;
        }
        let max_possible = (result.rules_evaluated as u32) * 8; // max severity + priority/32
        if max_possible > 0 {
            result.threat_score = ((total_severity * 100) / max_possible).min(100) as u8;
        }
    }

    result
}

/// Évalue une seule règle par index.
///
/// # Retour
/// - Un `EvalResult` avec le résultat.
pub fn evaluate_single(index: usize, data: &[u8]) -> EvalResult {
    let engine = RULE_ENGINE.lock();
    if index >= engine.count {
        return EvalResult::empty();
    }

    let rule = &engine.rules[index];
    let (matched, eval_count, match_count) = evaluate_rule(rule, data);

    EvalResult {
        rule_index: index,
        matched,
        action: rule.action,
        priority: rule.priority,
        severity: rule.severity,
        conditions_evaluated: eval_count,
        conditions_matched: match_count,
    }
}

/// Retourne le nombre de règles actives.
pub fn active_rule_count() -> u32 {
    RULE_COUNT.load(Ordering::Relaxed)
}

/// Statistiques du moteur de règles.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RuleEngineStats {
    pub active_rules: u32,
    pub total_evaluated: u64,
    pub total_matched: u64,
}

/// Retourne les statistiques du moteur.
pub fn get_stats() -> RuleEngineStats {
    RuleEngineStats {
        active_rules: RULE_COUNT.load(Ordering::Relaxed),
        total_evaluated: RULES_EVALUATED.load(Ordering::Relaxed),
        total_matched: RULES_MATCHED.load(Ordering::Relaxed),
    }
}

/// Réinitialise les statistiques.
pub fn reset_stats() {
    RULES_EVALUATED.store(0, Ordering::Relaxed);
    RULES_MATCHED.store(0, Ordering::Relaxed);
}

/// Initialise le moteur de règles.
pub fn yara_init() {
    let mut engine = RULE_ENGINE.lock();
    for i in 0..MAX_RULES {
        engine.rules[i] = Rule::empty();
    }
    engine.count = 0;
    RULE_COUNT.store(0, Ordering::Release);
    RULES_EVALUATED.store(0, Ordering::Release);
    RULES_MATCHED.store(0, Ordering::Release);
}
