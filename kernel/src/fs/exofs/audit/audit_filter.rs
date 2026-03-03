//! audit_filter.rs — Filtrage des entrées du journal d'audit ExoFS (no_std).
//!
//! `FilterCriteria` spécifie un ensemble de conditions combinées par AND.
//! `AuditFilter` applique les critères sur des listes ou des slices.
//!
//! Règles appliquées :
//!  - OOM-02   : try_reserve systématique
//!  - ARITH-02 : arithmétique vérifiée
//!  - RECUR-01 : zéro récursion

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::audit_entry::{AuditEntry, AuditOp, AuditResult, AuditSeverity};

// ─────────────────────────────────────────────────────────────────────────────
// FilterCriteria
// ─────────────────────────────────────────────────────────────────────────────

/// Ensemble de critères de filtrage combinés par AND logique.
///
/// Un champ `None` signifie « pas de contrainte sur ce champ ».
#[derive(Clone, Debug, Default)]
pub struct FilterCriteria {
    /// Filtre sur l'UID de l'acteur.
    pub actor_uid:    Option<u64>,
    /// Filtre sur l'objet cible.
    pub object_id:    Option<u64>,
    /// Filtre sur l'opération.
    pub op:           Option<AuditOp>,
    /// Filtre sur le résultat.
    pub result:       Option<AuditResult>,
    /// Sévérité minimale.
    pub min_severity: Option<AuditSeverity>,
    /// Plage de ticks (`from` inclus).
    pub tick_from:    Option<u64>,
    /// Plage de ticks (`to` inclus).
    pub tick_to:      Option<u64>,
    /// Si `true`, ne retourne que les entrées mutantes.
    pub only_mutating: bool,
    /// Si `true`, ne retourne que les entrées de sécurité.
    pub only_security: bool,
    /// Numéro de séquence minimum.
    pub seq_min:      Option<u64>,
    /// Numéro de séquence maximum.
    pub seq_max:      Option<u64>,
}

impl FilterCriteria {
    /// Critères vides (accepte tout).
    pub fn any() -> Self { Self::default() }

    /// Filtre sur l'UID de l'acteur.
    pub fn by_actor(uid: u64) -> Self {
        FilterCriteria { actor_uid: Some(uid), ..Default::default() }
    }

    /// Filtre sur l'opération.
    pub fn by_op(op: AuditOp) -> Self {
        FilterCriteria { op: Some(op), ..Default::default() }
    }

    /// Filtre sur le résultat.
    pub fn by_result(r: AuditResult) -> Self {
        FilterCriteria { result: Some(r), ..Default::default() }
    }

    /// Filtre sévérité minimale.
    pub fn min_severity(sev: AuditSeverity) -> Self {
        FilterCriteria { min_severity: Some(sev), ..Default::default() }
    }

    /// Filtre par plage de ticks.
    pub fn tick_range(from: u64, to: u64) -> Self {
        FilterCriteria { tick_from: Some(from), tick_to: Some(to), ..Default::default() }
    }

    /// Uniquement les entrées liées à la sécurité.
    pub fn security_only() -> Self {
        FilterCriteria { only_security: true, ..Default::default() }
    }

    /// Uniquement les entrées mutantes.
    pub fn mutating_only() -> Self {
        FilterCriteria { only_mutating: true, ..Default::default() }
    }

    // ── Builders par mutation ─────────────────────────────────────────────────

    pub fn with_actor(mut self, uid: u64)          -> Self { self.actor_uid    = Some(uid); self }
    pub fn with_object(mut self, id: u64)           -> Self { self.object_id   = Some(id);  self }
    pub fn with_op(mut self, op: AuditOp)           -> Self { self.op          = Some(op);  self }
    pub fn with_result(mut self, r: AuditResult)    -> Self { self.result      = Some(r);   self }
    pub fn with_min_sev(mut self, s: AuditSeverity) -> Self { self.min_severity = Some(s);  self }
    pub fn with_seq_min(mut self, s: u64)           -> Self { self.seq_min     = Some(s);   self }
    pub fn with_seq_max(mut self, s: u64)           -> Self { self.seq_max     = Some(s);   self }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditFilter
// ─────────────────────────────────────────────────────────────────────────────

/// Applique des critères de filtrage sur des entrées d'audit.
pub struct AuditFilter {
    criteria: FilterCriteria,
}

impl AuditFilter {
    /// Crée un filtre avec les critères donnés.
    pub fn new(criteria: FilterCriteria) -> Self { AuditFilter { criteria } }

    /// Crée un filtre qui accepte tout.
    pub fn passthrough() -> Self { AuditFilter::new(FilterCriteria::any()) }

    /// Retourne `true` si l'entrée correspond à tous les critères.
    pub fn matches(&self, e: &AuditEntry) -> bool {
        let c = &self.criteria;

        if let Some(uid) = c.actor_uid {
            if e.actor_uid != uid { return false; }
        }
        if let Some(oid) = c.object_id {
            if e.object_id != oid { return false; }
        }
        if let Some(op) = c.op {
            if e.op != op as u8 { return false; }
        }
        if let Some(res) = c.result {
            if e.result != res as u8 { return false; }
        }
        if let Some(sev) = c.min_severity {
            if e.severity < sev as u8 { return false; }
        }
        if let Some(from) = c.tick_from {
            if e.tick < from { return false; }
        }
        if let Some(to) = c.tick_to {
            if e.tick > to { return false; }
        }
        if c.only_mutating && !e.is_mutating() { return false; }
        if c.only_security && !e.is_security() { return false; }
        if let Some(smin) = c.seq_min {
            if e.seq < smin { return false; }
        }
        if let Some(smax) = c.seq_max {
            if e.seq > smax { return false; }
        }

        true
    }

    // ── Application sur des collections ──────────────────────────────────────

    /// Filtre un slice et retourne les entrées correspondantes.
    pub fn apply(&self, entries: &[AuditEntry]) -> ExofsResult<Vec<AuditEntry>> {
        let mut out: Vec<AuditEntry> = Vec::new();
        for e in entries {
            if self.matches(e) {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(*e);
            }
        }
        Ok(out)
    }

    /// Filtre et retourne les N premières entrées correspondantes.
    pub fn apply_limit(
        &self,
        entries: &[AuditEntry],
        max:     usize,
    ) -> ExofsResult<Vec<AuditEntry>> {
        let mut out: Vec<AuditEntry> = Vec::new();
        for e in entries {
            if out.len() >= max { break; }
            if self.matches(e) {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(*e);
            }
        }
        Ok(out)
    }

    /// Compte les entrées correspondantes sans les collecter.
    pub fn count(&self, entries: &[AuditEntry]) -> u64 {
        let mut n = 0u64;
        for e in entries {
            if self.matches(e) { n = n.wrapping_add(1); }
        }
        n
    }

    /// `true` si au moins une entrée correspond.
    pub fn any_match(&self, entries: &[AuditEntry]) -> bool {
        entries.iter().any(|e| self.matches(e))
    }

    /// Retourne les indices des entrées correspondantes.
    pub fn matching_indices(&self, entries: &[AuditEntry]) -> ExofsResult<Vec<usize>> {
        let mut out: Vec<usize> = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            if self.matches(e) {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(i);
            }
        }
        Ok(out)
    }

    /// Partitionne `entries` en (matching, non-matching).
    pub fn partition(
        &self,
        entries: &[AuditEntry],
    ) -> ExofsResult<(Vec<AuditEntry>, Vec<AuditEntry>)> {
        let mut yes: Vec<AuditEntry> = Vec::new();
        let mut no:  Vec<AuditEntry> = Vec::new();
        for e in entries {
            if self.matches(e) {
                yes.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                yes.push(*e);
            } else {
                no.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                no.push(*e);
            }
        }
        Ok((yes, no))
    }

    /// Accès aux critères courants.
    pub fn criteria(&self) -> &FilterCriteria { &self.criteria }

    /// Met à jour les critères.
    pub fn set_criteria(&mut self, c: FilterCriteria) { self.criteria = c; }
}

// ─────────────────────────────────────────────────────────────────────────────
// FilterChain — chaîne de filtres (AND-chaîning)
// ─────────────────────────────────────────────────────────────────────────────

/// Chaîne de filtres appliqués séquentiellement (AND logique).
pub struct FilterChain {
    filters: Vec<AuditFilter>,
}

impl FilterChain {
    /// Crée une chaîne vide (accepte tout).
    pub fn new() -> Self { FilterChain { filters: Vec::new() } }

    /// Ajoute un filtre à la chaîne.
    pub fn push(&mut self, f: AuditFilter) -> ExofsResult<()> {
        self.filters.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.filters.push(f);
        Ok(())
    }

    /// Teste si une entrée passe tous les filtres.
    pub fn matches(&self, e: &AuditEntry) -> bool {
        self.filters.iter().all(|f| f.matches(e))
    }

    /// Applique la chaîne sur un slice.
    pub fn apply(&self, entries: &[AuditEntry]) -> ExofsResult<Vec<AuditEntry>> {
        let mut out: Vec<AuditEntry> = Vec::new();
        for e in entries {
            if self.matches(e) {
                out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                out.push(*e);
            }
        }
        Ok(out)
    }

    /// Nombre de filtres dans la chaîne.
    pub fn len(&self) -> usize { self.filters.len() }

    /// `true` si la chaîne est vide.
    pub fn is_empty(&self) -> bool { self.filters.is_empty() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(op: AuditOp, result: AuditResult, uid: u64) -> AuditEntry {
        AuditEntry::new(100, uid, 0, 1, [0u8; 32], op, result, uid)
    }

    #[test] fn test_passthrough_matches_all() {
        let f = AuditFilter::passthrough();
        let e = entry(AuditOp::Read, AuditResult::Success, 1);
        assert!(f.matches(&e));
    }

    #[test] fn test_filter_by_actor() {
        let f = AuditFilter::new(FilterCriteria::by_actor(42));
        assert!( f.matches(&entry(AuditOp::Read, AuditResult::Success, 42)));
        assert!(!f.matches(&entry(AuditOp::Read, AuditResult::Success, 99)));
    }

    #[test] fn test_filter_by_op() {
        let f = AuditFilter::new(FilterCriteria::by_op(AuditOp::Delete));
        assert!( f.matches(&entry(AuditOp::Delete, AuditResult::Success, 1)));
        assert!(!f.matches(&entry(AuditOp::Read,   AuditResult::Success, 1)));
    }

    #[test] fn test_filter_by_result() {
        let f = AuditFilter::new(FilterCriteria::by_result(AuditResult::Denied));
        assert!( f.matches(&entry(AuditOp::Read, AuditResult::Denied,  1)));
        assert!(!f.matches(&entry(AuditOp::Read, AuditResult::Success, 1)));
    }

    #[test] fn test_filter_security_only() {
        let f = AuditFilter::new(FilterCriteria::security_only());
        assert!( f.matches(&entry(AuditOp::CryptoKey, AuditResult::Success, 1)));
        assert!(!f.matches(&entry(AuditOp::Read,      AuditResult::Success, 1)));
    }

    #[test] fn test_filter_mutating_only() {
        let f = AuditFilter::new(FilterCriteria::mutating_only());
        assert!( f.matches(&entry(AuditOp::Write, AuditResult::Success, 1)));
        assert!(!f.matches(&entry(AuditOp::Read,  AuditResult::Success, 1)));
    }

    #[test] fn test_apply_filters() {
        let entries = [
            entry(AuditOp::Read,  AuditResult::Success, 1),
            entry(AuditOp::Write, AuditResult::Success, 2),
            entry(AuditOp::Read,  AuditResult::Success, 3),
        ];
        let f = AuditFilter::new(FilterCriteria::by_op(AuditOp::Read));
        let out = f.apply(&entries).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test] fn test_apply_limit() {
        let entries: Vec<AuditEntry> = (0..10u64).map(|i|
            entry(AuditOp::Read, AuditResult::Success, i)
        ).collect();
        let f = AuditFilter::passthrough();
        let out = f.apply_limit(&entries, 3).unwrap();
        assert_eq!(out.len(), 3);
    }

    #[test] fn test_count() {
        let entries = [
            entry(AuditOp::Read, AuditResult::Denied,  1),
            entry(AuditOp::Read, AuditResult::Success, 2),
        ];
        let f = AuditFilter::new(FilterCriteria::by_result(AuditResult::Denied));
        assert_eq!(f.count(&entries), 1);
    }

    #[test] fn test_partition() {
        let entries = [
            entry(AuditOp::Write, AuditResult::Success, 1),
            entry(AuditOp::Read,  AuditResult::Success, 2),
        ];
        let f = AuditFilter::new(FilterCriteria::mutating_only());
        let (yes, no) = f.partition(&entries).unwrap();
        assert_eq!(yes.len(), 1);
        assert_eq!(no.len(), 1);
    }

    #[test] fn test_filter_chain_empty_accepts_all() {
        let chain = FilterChain::new();
        let e = entry(AuditOp::Read, AuditResult::Success, 1);
        assert!(chain.matches(&e));
    }

    #[test] fn test_filter_chain_and_logic() {
        let mut chain = FilterChain::new();
        chain.push(AuditFilter::new(FilterCriteria::mutating_only())).unwrap();
        chain.push(AuditFilter::new(FilterCriteria::by_result(AuditResult::Success))).unwrap();
        assert!( chain.matches(&entry(AuditOp::Write, AuditResult::Success, 1)));
        assert!(!chain.matches(&entry(AuditOp::Write, AuditResult::Denied,  1)));
        assert!(!chain.matches(&entry(AuditOp::Read,  AuditResult::Success, 1)));
    }

    #[test] fn test_filter_tick_range() {
        let mut e = entry(AuditOp::Read, AuditResult::Success, 1);
        // e.tick est fixé à 100 dans entry()
        let c = FilterCriteria::tick_range(50, 200);
        let f = AuditFilter::new(c);
        assert!(f.matches(&e));
        // Hors plage.
        let c2 = FilterCriteria::tick_range(200, 300);
        let f2 = AuditFilter::new(c2);
        assert!(!f2.matches(&e));
    }

    #[test] fn test_any_match_true() {
        let entries = [entry(AuditOp::Read, AuditResult::Success, 1)];
        let f = AuditFilter::passthrough();
        assert!(f.any_match(&entries));
    }

    #[test] fn test_any_match_false() {
        let entries = [entry(AuditOp::Read, AuditResult::Success, 1)];
        let f = AuditFilter::new(FilterCriteria::by_actor(999));
        assert!(!f.any_match(&entries));
    }

    #[test] fn test_matching_indices() {
        let entries = [
            entry(AuditOp::Read,  AuditResult::Success, 1),
            entry(AuditOp::Write, AuditResult::Success, 2),
            entry(AuditOp::Read,  AuditResult::Success, 3),
        ];
        let f = AuditFilter::new(FilterCriteria::by_op(AuditOp::Read));
        let idxs = f.matching_indices(&entries).unwrap();
        assert_eq!(idxs, [0, 2]);
    }

    #[test] fn test_set_criteria() {
        let mut f = AuditFilter::passthrough();
        f.set_criteria(FilterCriteria::by_actor(5));
        assert!(f.criteria().actor_uid == Some(5));
    }

    #[test] fn test_filter_chain_len() {
        let mut c = FilterChain::new();
        assert!(c.is_empty());
        c.push(AuditFilter::passthrough()).unwrap();
        assert_eq!(c.len(), 1);
    }

    #[test] fn test_filter_seq_range() {
        let entries: Vec<_> = (0u64..5).map(|i| {
            let mut e = entry(AuditOp::Read, AuditResult::Success, 1);
            e.seq = i;
            e
        }).collect();
        let c = FilterCriteria { seq_min: Some(2), seq_max: Some(3), ..Default::default() };
        let f = AuditFilter::new(c);
        let out = f.apply(&entries).unwrap();
        assert_eq!(out.len(), 2);
    }
}
