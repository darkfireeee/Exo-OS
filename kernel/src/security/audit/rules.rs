// kernel/src/security/audit/rules.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Audit Rules — Règles de filtrage et de déclenchement d'audit
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Set de 64 règles maximum (taille fixe, pas d'allocation)
//   • Chaque règle filtre par : pid, uid, syscall_nr, catégorie, outcome
//   • Les règles sont évaluées en ordre de priorité (0 = plus haute priorité)
//   • Une règle peut déclencher : LOG, ALERT, KILL, DENY
//
// RÈGLE ARULE-01 : L'ordre d'évaluation est déterministe (index croissant).
// RÈGLE ARULE-02 : Pas de règle ne peut désactiver SecurityViolation.
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use super::logger::AuditCategory;

// ─────────────────────────────────────────────────────────────────────────────
// Action d'une règle
// ─────────────────────────────────────────────────────────────────────────────

/// Action déclenchée par une règle d'audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RuleAction {
    /// Ne rien faire (règle de suppression).
    Skip   = 0,
    /// Enregistrer l'événement.
    Log    = 1,
    /// Enregistrer et générer une alerte.
    Alert  = 2,
    /// Refuser l'opération.
    Deny   = 3,
    /// Tuer le processus.
    Kill   = 4,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditRule — règle individuelle
// ─────────────────────────────────────────────────────────────────────────────

/// Règle d'audit individuelle.
///
/// Tous les champs `Option::None` signifient "match all" pour ce critère.
#[derive(Clone, Copy)]
pub struct AuditRule {
    /// PID à surveiller (None = tous les PID).
    pub pid:           Option<u32>,
    /// UID à surveiller (None = tous les UID).
    pub uid:           Option<u32>,
    /// Numéro de syscall (None = tous les syscalls).
    pub syscall_nr:    Option<u32>,
    /// Catégorie (None = toutes).
    pub category:      Option<AuditCategory>,
    /// Ne s'applique qu'aux outcomes donnés (bitmask sur AuditOutcome u8).
    /// 0xFF = tous.
    pub outcome_mask:  u8,
    /// Action à déclencher si la règle matche.
    pub action:        RuleAction,
    /// La règle est-elle active ?
    pub enabled:       bool,
    /// Priorité (0 = plus haute).
    pub priority:      u8,
    /// Compteur de matches.
    pub match_count:   u64,
}

impl AuditRule {
    pub const fn new_log_all() -> Self {
        Self {
            pid:          None,
            uid:          None,
            syscall_nr:   None,
            category:     None,
            outcome_mask: 0xFF,
            action:       RuleAction::Log,
            enabled:      true,
            priority:     128,
            match_count:  0,
        }
    }

    pub const fn new_deny_pid(pid: u32) -> Self {
        Self {
            pid:          Some(pid),
            uid:          None,
            syscall_nr:   None,
            category:     None,
            outcome_mask: 0xFF,
            action:       RuleAction::Deny,
            enabled:      true,
            priority:     10,
            match_count:  0,
        }
    }

    pub const fn new_alert_uid(uid: u32) -> Self {
        Self {
            pid:          None,
            uid:          Some(uid),
            syscall_nr:   None,
            category:     None,
            outcome_mask: 0xFF,
            action:       RuleAction::Alert,
            enabled:      true,
            priority:     20,
            match_count:  0,
        }
    }

    pub const fn new_log_syscall(nr: u32) -> Self {
        Self {
            pid:          None,
            uid:          None,
            syscall_nr:   Some(nr),
            category:     None,
            outcome_mask: 0xFF,
            action:       RuleAction::Log,
            enabled:      true,
            priority:     50,
            match_count:  0,
        }
    }

    /// Teste si cette règle s'applique à l'événement décrit.
    pub fn matches(&self, pid: u32, uid: u32, syscall_nr: u32, category: AuditCategory, outcome: u8) -> bool {
        if !self.enabled { return false; }
        // RÈGLE ARULE-02 : SecurityViolation ne peut pas être supprimée
        if category == AuditCategory::SecurityViolation && self.action == RuleAction::Skip {
            return false;
        }
        if let Some(p) = self.pid       { if p != pid        { return false; } }
        if let Some(u) = self.uid       { if u != uid        { return false; } }
        if let Some(s) = self.syscall_nr { if s != syscall_nr { return false; } }
        if let Some(c) = self.category  { if c as u8 != category as u8 { return false; } }
        if self.outcome_mask != 0xFF && (self.outcome_mask & (1u8 << outcome)) == 0 {
            return false;
        }
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RuleSet — ensemble de règles
// ─────────────────────────────────────────────────────────────────────────────

const MAX_RULES: usize = 64;
const NONE_RULE: Option<AuditRule> = None;

pub struct RuleSet {
    rules: [Option<AuditRule>; MAX_RULES],
    count: usize,
}

impl RuleSet {
    pub const fn new() -> Self {
        Self {
            rules: [NONE_RULE; MAX_RULES],
            count: 0,
        }
    }

    /// Ajoute une règle au RuleSet.
    ///
    /// Retourne l'index de la règle ou `Err` si le set est plein.
    pub fn add_rule(&mut self, rule: AuditRule) -> Result<usize, ()> {
        if self.count >= MAX_RULES {
            return Err(());
        }
        // Trouver le premier slot libre
        for i in 0..MAX_RULES {
            if self.rules[i].is_none() {
                self.rules[i] = Some(rule);
                self.count += 1;
                return Ok(i);
            }
        }
        Err(())
    }

    /// Supprime la règle à l'index `idx`.
    pub fn remove_rule(&mut self, idx: usize) -> bool {
        if idx >= MAX_RULES || self.rules[idx].is_none() {
            return false;
        }
        self.rules[idx] = None;
        if self.count > 0 { self.count -= 1; }
        true
    }

    /// Active ou désactive la règle à `idx`.
    pub fn set_enabled(&mut self, idx: usize, enabled: bool) -> bool {
        if let Some(r) = &mut self.rules[idx] {
            r.enabled = enabled;
            true
        } else {
            false
        }
    }

    /// Évalue toutes les règles contre un événement.
    ///
    /// RÈGLE ARULE-01 : Évaluation par index croissant, première règle matchante prime.
    ///
    /// Retourne `(action, rule_index)` ou `(Log, usize::MAX)` si aucune règle.
    pub fn evaluate(
        &mut self,
        pid:        u32,
        uid:        u32,
        syscall_nr: u32,
        category:   AuditCategory,
        outcome:    u8,
    ) -> (RuleAction, usize) {
        // Trier par priorité (chercher la règle de plus haute priorité qui matche)
        let mut best_priority = u8::MAX;
        let mut best_action   = RuleAction::Log;
        let mut best_idx      = usize::MAX;

        for i in 0..MAX_RULES {
            if let Some(r) = &mut self.rules[i] {
                if r.matches(pid, uid, syscall_nr, category, outcome) {
                    if r.priority < best_priority {
                        best_priority = r.priority;
                        best_action   = r.action;
                        best_idx      = i;
                    }
                }
            }
        }

        // Incrémenter le compteur de la règle gagnante
        if best_idx < MAX_RULES {
            if let Some(r) = &mut self.rules[best_idx] {
                r.match_count = r.match_count.wrapping_add(1);
            }
        }

        (best_action, best_idx)
    }

    pub fn count(&self) -> usize { self.count }
}

// ─────────────────────────────────────────────────────────────────────────────
// Jeu de règles global
// ─────────────────────────────────────────────────────────────────────────────

static GLOBAL_RULES: spin::Mutex<RuleSet> = spin::Mutex::new(RuleSet::new());
static RULE_EVALS:   AtomicU64 = AtomicU64::new(0);
static RULE_MATCHES: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Ajoute une règle globale.
pub fn add_global_rule(rule: AuditRule) -> Result<usize, ()> {
    GLOBAL_RULES.lock().add_rule(rule)
}

/// Supprime une règle globale.
pub fn remove_global_rule(idx: usize) -> bool {
    GLOBAL_RULES.lock().remove_rule(idx)
}

/// Évalue les règles globales contre un événement.
pub fn evaluate_global(
    pid:        u32,
    uid:        u32,
    syscall_nr: u32,
    category:   AuditCategory,
    outcome:    u8,
) -> RuleAction {
    RULE_EVALS.fetch_add(1, Ordering::Relaxed);
    let (action, idx) = GLOBAL_RULES.lock().evaluate(pid, uid, syscall_nr, category, outcome);
    if idx != usize::MAX {
        RULE_MATCHES.fetch_add(1, Ordering::Relaxed);
    }
    action
}

#[derive(Debug, Clone, Copy)]
pub struct RuleStats {
    pub evaluations: u64,
    pub matches:     u64,
    pub rule_count:  usize,
}

pub fn rule_stats() -> RuleStats {
    RuleStats {
        evaluations: RULE_EVALS.load(Ordering::Relaxed),
        matches:     RULE_MATCHES.load(Ordering::Relaxed),
        rule_count:  GLOBAL_RULES.lock().count(),
    }
}
