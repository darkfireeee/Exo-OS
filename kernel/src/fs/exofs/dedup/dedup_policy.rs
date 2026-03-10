//! DedupPolicy — politiques de déduplication configurables (no_std).
//!
//! Définit quand et comment la déduplication est appliquée :
//! mode inline/background/désactivé, seuils, limites, priorité.
//!
//! RECUR-01 : aucune récursion.
//! OOM-02   : try_reserve.
//! ARITH-02 : saturating / checked / wrapping.


use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes par défaut
// ─────────────────────────────────────────────────────────────────────────────

pub const POLICY_DEFAULT_MIN_BLOB_SIZE:       u64  = 4096;    // 4 KiB
pub const POLICY_DEFAULT_MAX_CHUNK_SIZE:      usize = 65536;  // 64 KiB
pub const POLICY_DEFAULT_SIMILARITY_PCT:      u8    = 80;     // 80%
pub const POLICY_DEFAULT_INLINE_SIZE_LIMIT:   u64   = 16 * 1024 * 1024; // 16 MiB
pub const POLICY_MAX_STACK_DEPTH:             usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
// Enums
// ─────────────────────────────────────────────────────────────────────────────

/// Mode de déduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupMode {
    /// Déduplication synchrone lors de l'écriture.
    Inline,
    /// Déduplication asynchrone en arrière-plan.
    Background,
    /// Désactivé.
    Disabled,
}

impl DedupMode {
    pub fn is_active(&self) -> bool { !matches!(self, DedupMode::Disabled) }
    pub fn name(&self) -> &'static str {
        match self {
            DedupMode::Inline     => "inline",
            DedupMode::Background => "background",
            DedupMode::Disabled   => "disabled",
        }
    }
}

/// Priorité de déduplication (affecte l'ordre de traitement en mode Background).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DedupPriority {
    Low    = 0,
    Normal = 1,
    High   = 2,
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupPolicy
// ─────────────────────────────────────────────────────────────────────────────

/// Politique de déduplication complète.
#[derive(Debug, Clone)]
pub struct DedupPolicy {
    pub mode:                  DedupMode,
    pub min_blob_size:         u64,
    pub max_chunk_size:        usize,
    pub similarity_threshold:  u8,
    pub inline_size_limit:     u64,
    pub skip_encrypted:        bool,
    pub skip_compressed:       bool,
    pub priority:              DedupPriority,
    pub enable_delta:          bool,  // delta compression entre chunks similaires.
    pub verify_on_dedup:       bool,  // recompute le blake3 après dédup.
}

impl DedupPolicy {
    pub fn default() -> Self {
        Self {
            mode:                 DedupMode::Inline,
            min_blob_size:        POLICY_DEFAULT_MIN_BLOB_SIZE,
            max_chunk_size:       POLICY_DEFAULT_MAX_CHUNK_SIZE,
            similarity_threshold: POLICY_DEFAULT_SIMILARITY_PCT,
            inline_size_limit:    POLICY_DEFAULT_INLINE_SIZE_LIMIT,
            skip_encrypted:       true,
            skip_compressed:      false,
            priority:             DedupPriority::Normal,
            enable_delta:         false,
            verify_on_dedup:      true,
        }
    }

    pub fn disabled() -> Self {
        Self { mode: DedupMode::Disabled, ..Self::default() }
    }

    pub fn background() -> Self {
        Self { mode: DedupMode::Background, ..Self::default() }
    }

    /// Valide la politique.
    pub fn validate(&self) -> ExofsResult<()> {
        if self.min_blob_size == 0            { return Err(ExofsError::InvalidArgument); }
        if self.max_chunk_size == 0           { return Err(ExofsError::InvalidArgument); }
        if self.similarity_threshold > 100    { return Err(ExofsError::InvalidArgument); }
        if self.inline_size_limit == 0        { return Err(ExofsError::InvalidArgument); }
        Ok(())
    }

    /// Un blob de cette taille doit-il être dédupliqué ?
    pub fn should_dedup(&self, blob_size: u64) -> bool {
        self.mode.is_active() && blob_size >= self.min_blob_size
    }

    /// La déduplication doit-elle être inline pour cette taille ?
    pub fn should_inline(&self, blob_size: u64) -> bool {
        matches!(self.mode, DedupMode::Inline) && blob_size <= self.inline_size_limit
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupPolicyRule — règle unitaire composant un moteur de politiques
// ─────────────────────────────────────────────────────────────────────────────

/// Condition de déclenchement d'une règle.
#[derive(Debug, Clone, Copy)]
pub enum PolicyCondition {
    BlobSizeAbove(u64),
    BlobSizeBelow(u64),
    AlwaysTrue,
    AlwaysFalse,
}

impl PolicyCondition {
    pub fn matches(&self, blob_size: u64) -> bool {
        match self {
            PolicyCondition::BlobSizeAbove(t) => blob_size > *t,
            PolicyCondition::BlobSizeBelow(t) => blob_size < *t,
            PolicyCondition::AlwaysTrue        => true,
            PolicyCondition::AlwaysFalse       => false,
        }
    }
}

/// Règle unitaire : condition → politique.
#[derive(Debug, Clone)]
pub struct DedupPolicyRule {
    pub condition: PolicyCondition,
    pub policy:    DedupPolicy,
    pub name:      &'static str,
}

impl DedupPolicyRule {
    pub fn new(condition: PolicyCondition, policy: DedupPolicy, name: &'static str) -> Self {
        Self { condition, policy, name }
    }

    pub fn matches(&self, blob_size: u64) -> bool {
        self.condition.matches(blob_size)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupPolicyEngine — moteur d'évaluation des politiques
// ─────────────────────────────────────────────────────────────────────────────

/// Pile de règles de politiques.
pub struct DedupPolicyEngine {
    rules:   Vec<DedupPolicyRule>,
    default: DedupPolicy,
}

impl DedupPolicyEngine {
    pub fn new(default: DedupPolicy) -> Self {
        Self { rules: Vec::new(), default }
    }

    /// Ajoute une règle à la pile.
    ///
    /// OOM-02 : try_reserve.
    pub fn push_rule(&mut self, rule: DedupPolicyRule) -> ExofsResult<()> {
        if self.rules.len() >= POLICY_MAX_STACK_DEPTH {
            return Err(ExofsError::NoMemory);
        }
        self.rules.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.rules.push(rule);
        Ok(())
    }

    /// Évalue les règles dans l'ordre et retourne la première politique correspondante.
    ///
    /// RECUR-01 : boucle for — pas de récursion.
    pub fn evaluate(&self, blob_size: u64) -> &DedupPolicy {
        for rule in &self.rules {
            if rule.matches(blob_size) {
                return &rule.policy;
            }
        }
        &self.default
    }

    /// Retourne vrai si au moins une règle s'applique.
    pub fn has_match(&self, blob_size: u64) -> bool {
        self.rules.iter().any(|r| r.matches(blob_size))
    }

    pub fn rule_count(&self) -> usize { self.rules.len() }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupPolicyPreset — politiques prédéfinies courantes
// ─────────────────────────────────────────────────────────────────────────────

/// Politiques prédéfinies.
pub struct DedupPolicyPreset;

impl DedupPolicyPreset {
    /// Politique agressive : déduplique tout, y compris les petits blobs.
    pub fn aggressive() -> DedupPolicy {
        DedupPolicy {
            mode:                 DedupMode::Inline,
            min_blob_size:        512,
            max_chunk_size:       8192,
            similarity_threshold: 60,
            inline_size_limit:    64 * 1024 * 1024,
            skip_encrypted:       false,
            skip_compressed:      false,
            priority:             DedupPriority::High,
            enable_delta:         true,
            verify_on_dedup:      true,
        }
    }

    /// Politique conservatrice : seulement les gros blobs.
    pub fn conservative() -> DedupPolicy {
        DedupPolicy {
            mode:                 DedupMode::Background,
            min_blob_size:        1024 * 1024,
            max_chunk_size:       65536,
            similarity_threshold: 95,
            inline_size_limit:    4 * 1024 * 1024,
            skip_encrypted:       true,
            skip_compressed:      true,
            priority:             DedupPriority::Low,
            enable_delta:         false,
            verify_on_dedup:      true,
        }
    }

    /// Politique rapide : optimisée pour la vitesse, vérifie moins.
    pub fn fast() -> DedupPolicy {
        DedupPolicy {
            mode:                 DedupMode::Inline,
            min_blob_size:        4096,
            max_chunk_size:       65536,
            similarity_threshold: 70,
            inline_size_limit:    8 * 1024 * 1024,
            skip_encrypted:       true,
            skip_compressed:      true,
            priority:             DedupPriority::Normal,
            enable_delta:         false,
            verify_on_dedup:      false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn test_policy_default_valid() {
        assert!(DedupPolicy::default().validate().is_ok());
    }

    #[test] fn test_policy_disabled_not_active() {
        let p = DedupPolicy::disabled();
        assert!(!p.mode.is_active());
        assert!(!p.should_dedup(10 * 1024 * 1024));
    }

    #[test] fn test_should_dedup_below_min_size() {
        let p = DedupPolicy::default();
        assert!(!p.should_dedup(100));
    }

    #[test] fn test_should_dedup_above_min_size() {
        let p = DedupPolicy::default();
        assert!(p.should_dedup(POLICY_DEFAULT_MIN_BLOB_SIZE));
    }

    #[test] fn test_should_inline() {
        let p = DedupPolicy::default();
        assert!(p.should_inline(1024));
        assert!(!p.should_inline(POLICY_DEFAULT_INLINE_SIZE_LIMIT + 1));
    }

    #[test] fn test_engine_default_returned_when_no_rules() {
        let eng = DedupPolicyEngine::new(DedupPolicy::disabled());
        let p   = eng.evaluate(1024 * 1024);
        assert!(!p.mode.is_active());
    }

    #[test] fn test_engine_first_matching_rule() {
        let mut eng = DedupPolicyEngine::new(DedupPolicy::disabled());
        eng.push_rule(DedupPolicyRule::new(
            PolicyCondition::BlobSizeAbove(4096),
            DedupPolicy::default(),
            "big",
        )).unwrap();
        let p = eng.evaluate(1024 * 1024);
        assert!(p.mode.is_active());
        let p2 = eng.evaluate(100);
        assert!(!p2.mode.is_active());
    }

    #[test] fn test_engine_max_stack() {
        let mut eng = DedupPolicyEngine::new(DedupPolicy::default());
        for i in 0..POLICY_MAX_STACK_DEPTH {
            let _ = i;
            eng.push_rule(DedupPolicyRule::new(
                PolicyCondition::AlwaysFalse,
                DedupPolicy::disabled(),
                "noop",
            )).unwrap();
        }
        let overflow = eng.push_rule(DedupPolicyRule::new(
            PolicyCondition::AlwaysFalse, DedupPolicy::disabled(), "overflow",
        ));
        assert!(overflow.is_err());
    }

    #[test] fn test_presets_valid() {
        assert!(DedupPolicyPreset::aggressive().validate().is_ok());
        assert!(DedupPolicyPreset::conservative().validate().is_ok());
        assert!(DedupPolicyPreset::fast().validate().is_ok());
    }

    #[test] fn test_dedup_mode_name() {
        assert_eq!(DedupMode::Inline.name(), "inline");
        assert_eq!(DedupMode::Background.name(), "background");
        assert_eq!(DedupMode::Disabled.name(), "disabled");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DedupPolicyReport — rapport sur la politique active
// ─────────────────────────────────────────────────────────────────────────────

/// Rapport lisible de la politique de déduplication active.
#[derive(Debug, Clone)]
pub struct DedupPolicyReport {
    pub mode_name:            &'static str,
    pub is_active:            bool,
    pub min_blob_size:        u64,
    pub max_chunk_size:       usize,
    pub similarity_threshold: u8,
    pub skip_encrypted:       bool,
    pub skip_compressed:      bool,
    pub verify_on_dedup:      bool,
    pub enable_delta:         bool,
}

impl DedupPolicyReport {
    pub fn from_policy(p: &DedupPolicy) -> Self {
        Self {
            mode_name:            p.mode.name(),
            is_active:            p.mode.is_active(),
            min_blob_size:        p.min_blob_size,
            max_chunk_size:       p.max_chunk_size,
            similarity_threshold: p.similarity_threshold,
            skip_encrypted:       p.skip_encrypted,
            skip_compressed:      p.skip_compressed,
            verify_on_dedup:      p.verify_on_dedup,
            enable_delta:         p.enable_delta,
        }
    }
}

impl DedupPolicyEngine {
    /// Génère un rapport pour la politique qui s'appliquerait au blob donné.
    pub fn report_for_size(&self, blob_size: u64) -> DedupPolicyReport {
        DedupPolicyReport::from_policy(self.evaluate(blob_size))
    }
}

#[cfg(test)]
mod tests_report {
    use super::*;

    #[test] fn test_report_from_default() {
        let r = DedupPolicyReport::from_policy(&DedupPolicy::default());
        assert!(r.is_active);
        assert_eq!(r.mode_name, "inline");
    }

    #[test] fn test_report_from_disabled() {
        let r = DedupPolicyReport::from_policy(&DedupPolicy::disabled());
        assert!(!r.is_active);
    }

    #[test] fn test_engine_report_for_size() {
        let eng = DedupPolicyEngine::new(DedupPolicy::default());
        let r   = eng.report_for_size(1024 * 1024);
        assert!(r.is_active);
    }

    #[test] fn test_condition_always_true() {
        assert!(PolicyCondition::AlwaysTrue.matches(0));
        assert!(PolicyCondition::AlwaysTrue.matches(u64::MAX));
    }

    #[test] fn test_condition_size_above() {
        let c = PolicyCondition::BlobSizeAbove(1000);
        assert!(!c.matches(1000));
        assert!(c.matches(1001));
    }
}
