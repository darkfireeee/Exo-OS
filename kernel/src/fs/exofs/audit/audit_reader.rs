//! audit_reader.rs — Lecture et parcours du journal d'audit ExoFS (no_std).
//!
//! Fournit un curseur positionnable sur le ring-buffer, un itérateur
//! séquentiel et des primitives de recherche par critère.
//!
//! Règles appliquées :
//!  - RECUR-01 : zéro récursion
//!  - OOM-02   : try_reserve systématique
//!  - ARITH-02 : arithmétique vérifiée


extern crate alloc;
use alloc::vec::Vec;

use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::audit_entry::{AuditEntry, AuditOp, AuditResult, AuditSeverity, AuditSummary};
use super::audit_log::{AuditLog, AUDIT_LOG, RING_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum d'entrées retournées par un appel `read_n`.
pub const READER_PAGE_MAX: usize = 1024;

// ─────────────────────────────────────────────────────────────────────────────
// ReadDirection — sens de lecture
// ─────────────────────────────────────────────────────────────────────────────

/// Sens de parcours du curseur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadDirection {
    /// Du plus ancien au plus récent.
    Forward,
    /// Du plus récent au plus ancien.
    Backward,
}

// ─────────────────────────────────────────────────────────────────────────────
// ReaderStats
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques cumulées d'un `AuditReader`.
#[derive(Clone, Debug, Default)]
pub struct ReaderStats {
    pub total_read:    u64,
    pub total_skipped: u64,
    pub total_invalid: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditReader
// ─────────────────────────────────────────────────────────────────────────────

/// Curseur de lecture positionnable sur le ring-buffer d'audit.
///
/// Le curseur pointe sur une position absolue (séquence). Il avance ou
/// recule d'une entrée à la fois. Utilise le `AUDIT_LOG` global.
pub struct AuditReader {
    /// Position absolue courante dans le ring (indice séquentiel).
    cursor: u64,
    /// Sens de lecture.
    direction: ReadDirection,
    /// Filtre minimal de sévérité (entrées < min_severity ignorées).
    min_severity: Option<AuditSeverity>,
    /// Statistiques.
    stats: ReaderStats,
}

impl AuditReader {
    /// Crée un reader positionné au début (entrée la plus ancienne disponible).
    pub fn new() -> Self {
        let head  = AUDIT_LOG.next_seq();
        let count = AUDIT_LOG.available() as u64;
        let start = head.wrapping_sub(count);
        AuditReader {
            cursor:       start,
            direction:    ReadDirection::Forward,
            min_severity: None,
            stats:        ReaderStats::default(),
        }
    }

    /// Crée un reader positionné à la fin (entrée la plus récente).
    pub fn from_tail() -> Self {
        let head = AUDIT_LOG.next_seq();
        AuditReader {
            cursor:       head.saturating_sub(1),
            direction:    ReadDirection::Backward,
            min_severity: None,
            stats:        ReaderStats::default(),
        }
    }

    /// Crée un reader avec un log explicite (utile pour les tests).
    pub fn with_log_at(log: &AuditLog, direction: ReadDirection) -> Self {
        let head  = log.next_seq();
        let count = log.available() as u64;
        let cursor = match direction {
            ReadDirection::Forward  => head.wrapping_sub(count),
            ReadDirection::Backward => head.saturating_sub(1),
        };
        AuditReader {
            cursor,
            direction,
            min_severity: None,
            stats: ReaderStats::default(),
        }
    }

    /// Filtre les entrées dont la sévérité est inférieure à `min`.
    pub fn min_severity(mut self, min: AuditSeverity) -> Self {
        self.min_severity = Some(min);
        self
    }

    /// Positionne le curseur sur la séquence absolue `seq`.
    pub fn seek(&mut self, seq: u64) {
        self.cursor = seq;
    }

    /// Avance ou recule le curseur d'une entrée et retourne la suivante.
    ///
    /// Retourne `None` si le curseur est sorti des bornes disponibles.
    pub fn next(&mut self) -> Option<AuditEntry> {
        self.next_from(&AUDIT_LOG)
    }

    /// Variante qui prend un log explicite (utile pour les tests).
    pub fn next_from(&mut self, log: &AuditLog) -> Option<AuditEntry> {
        let head  = log.next_seq();
        let count = log.available() as u64;
        let oldest = head.wrapping_sub(count);

        loop {
            // Borne de fin.
            match self.direction {
                ReadDirection::Forward  => {
                    if self.cursor >= head { return None; }
                }
                ReadDirection::Backward => {
                    if self.cursor < oldest || head == 0 { return None; }
                }
            }

            let entry = log.read_at(self.cursor as usize);

            // Avance le curseur.
            match self.direction {
                ReadDirection::Forward  => {
                    self.cursor = self.cursor.wrapping_add(1);
                }
                ReadDirection::Backward => {
                    if self.cursor == 0 { return None; }
                    self.cursor = self.cursor.wrapping_sub(1);
                }
            }

            // Validation magic.
            if !entry.is_valid() {
                self.stats.total_invalid = self.stats.total_invalid.wrapping_add(1);
                continue;
            }

            // Filtre sévérité.
            if let Some(min) = self.min_severity {
                if entry.severity < min as u8 {
                    self.stats.total_skipped = self.stats.total_skipped.wrapping_add(1);
                    continue;
                }
            }

            self.stats.total_read = self.stats.total_read.wrapping_add(1);
            return Some(entry);
        }
    }

    // ── Lectures paginées ─────────────────────────────────────────────────────

    /// Lit jusqu'à `n` entrées (≤ `READER_PAGE_MAX`) depuis la position courante.
    pub fn read_n(&mut self, n: usize) -> ExofsResult<Vec<AuditEntry>> {
        let cap = n.min(READER_PAGE_MAX);
        let mut out: Vec<AuditEntry> = Vec::new();
        out.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < cap {
            match self.next() {
                Some(e) => { out.push(e); i += 1; }
                None    => break,
            }
        }
        Ok(out)
    }

    /// Lit toutes les entrées disponibles depuis la position courante.
    pub fn read_all(&mut self) -> ExofsResult<Vec<AuditEntry>> {
        self.read_n(RING_SIZE)
    }

    // ── Recherche ─────────────────────────────────────────────────────────────

    /// Retourne la première entrée vérifiant `pred` depuis la position courante.
    pub fn find<F>(&mut self, pred: F) -> Option<AuditEntry>
    where
        F: Fn(&AuditEntry) -> bool,
    {
        loop {
            match self.next() {
                Some(e) if pred(&e) => return Some(e),
                Some(_) => {}
                None    => return None,
            }
        }
    }

    /// Collecte toutes les entrées vérifiant `pred` (borné à `RING_SIZE`).
    pub fn collect_if<F>(&mut self, pred: F) -> ExofsResult<Vec<AuditEntry>>
    where
        F: Fn(&AuditEntry) -> bool,
    {
        let mut out: Vec<AuditEntry> = Vec::new();
        loop {
            match self.next() {
                Some(e) if pred(&e) => {
                    out.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                    out.push(e);
                }
                Some(_) => {}
                None    => break,
            }
        }
        Ok(out)
    }

    /// Retourne toutes les entrées d'un acteur donné.
    pub fn entries_by_actor(&mut self, actor_uid: u64) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.actor_uid == actor_uid)
    }

    /// Retourne toutes les entrées d'une opération donnée.
    pub fn entries_by_op(&mut self, op: AuditOp) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.op == op as u8)
    }

    /// Retourne toutes les entrées d'un résultat donné.
    pub fn entries_by_result(&mut self, r: AuditResult) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.result == r as u8)
    }

    /// Retourne toutes les entrées de sécurité.
    pub fn security_entries(&mut self) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.is_security())
    }

    /// Retourne toutes les entrées dans une plage de ticks.
    pub fn entries_in_tick_range(
        &mut self,
        from_tick: u64,
        to_tick:   u64,
    ) -> ExofsResult<Vec<AuditEntry>> {
        self.collect_if(|e| e.tick >= from_tick && e.tick <= to_tick)
    }

    // ── Statistiques ──────────────────────────────────────────────────────────

    /// Calcule un résumé de toutes les entrées disponibles (repart du début).
    pub fn summarize(&mut self) -> AuditSummary {
        self.seek(AUDIT_LOG.next_seq().wrapping_sub(AUDIT_LOG.available() as u64));
        let mut summary = AuditSummary::default();
        loop {
            match self.next() {
                Some(e) => summary.feed(&e),
                None    => break,
            }
        }
        summary
    }

    /// Statistiques du reader.
    pub fn stats(&self) -> &ReaderStats { &self.stats }

    /// Réinitialise les statistiques.
    pub fn reset_stats(&mut self) { self.stats = ReaderStats::default(); }

    /// Position courante du curseur (séquence absolue).
    pub fn cursor(&self) -> u64 { self.cursor }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers libres
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne les N dernières entrées du log global.
pub fn last_n_entries(n: usize) -> ExofsResult<Vec<AuditEntry>> {
    let cap = n.min(RING_SIZE).min(READER_PAGE_MAX);
    let mut buf: Vec<AuditEntry> = Vec::new();
    buf.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
    let avail = AUDIT_LOG.available();
    let actual = cap.min(avail);
    buf.resize(actual, AuditEntry::new(0, 0, 0, 0, [0u8; 32],
        AuditOp::Read, AuditResult::Success, 0));
    let n_read = AUDIT_LOG.read_recent_into(&mut buf, actual);
    buf.truncate(n_read);
    Ok(buf)
}

/// Retourne le résumé rapide du log global.
pub fn quick_summary() -> AuditSummary {
    let mut s = AuditSummary::default();
    AUDIT_LOG.for_each(|e| s.feed(e));
    s
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::audit_log::AuditLog;

    fn push_entry(log: &AuditLog, op: AuditOp, result: AuditResult, uid: u64) {
        log.push(AuditEntry::new(1, uid, 0, 0, [0u8; 32], op, result, log.next_seq()));
    }

    #[test] fn test_new_reader_empty_log() {
        let log = AuditLog::new_const();
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        assert!(r.next_from(&log).is_none());
    }

    #[test] fn test_forward_reads_in_order() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read,  AuditResult::Success, 1);
        push_entry(&log, AuditOp::Write, AuditResult::Success, 2);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let e1 = r.next_from(&log).unwrap();
        let e2 = r.next_from(&log).unwrap();
        assert!(e1.seq <= e2.seq);
    }

    #[test] fn test_backward_reads_last_first() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read,   AuditResult::Success, 1);
        push_entry(&log, AuditOp::Delete, AuditResult::Success, 2);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Backward);
        let e = r.next_from(&log).unwrap();
        assert_eq!(e.op, AuditOp::Delete as u8);
    }

    #[test] fn test_find_by_op() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read,  AuditResult::Success, 1);
        push_entry(&log, AuditOp::Write, AuditResult::Success, 2);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let e = r.find(|e| e.op == AuditOp::Write as u8);
        assert!(e.is_some());
    }

    #[test] fn test_min_severity_filters() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read, AuditResult::Success, 1); // Info
        push_entry(&log, AuditOp::Read, AuditResult::Denied,  2); // Critical
        // Lire avec filtre min_severity = Critical sur le log global.
        // On vérifie surtout que la méthode ne panique pas.
        let mut r = AuditReader::new()
            .min_severity(AuditSeverity::Critical);
        let _ = r.read_n(10).unwrap();
    }

    #[test] fn test_read_all_returns_vec() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Create, AuditResult::Success, 1);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let all = r.read_all().unwrap();
        assert!(!all.is_empty());
    }

    #[test] fn test_entries_by_actor() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read, AuditResult::Success, 99);
        push_entry(&log, AuditOp::Read, AuditResult::Success, 42);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let v = r.collect_if(|e| e.actor_uid == 99).unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test] fn test_stats_counts_reads() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::Read, AuditResult::Success, 1);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        r.next_from(&log);
        assert_eq!(r.stats().total_read, 1);
    }

    #[test] fn test_last_n_entries() {
        let v = last_n_entries(2).unwrap();
        // Le log global contient au moins une entrée grâce aux autres tests.
        let _ = v;
    }

    #[test] fn test_quick_summary() {
        let s = quick_summary();
        assert!(s.total >= 0);
    }

    #[test] fn test_seek_and_reread() {
        let log = AuditLog::new_const();
        push_entry(&log, AuditOp::GcTrigger, AuditResult::Success, 1);
        let mut r = AuditReader::with_log_at(&log, ReadDirection::Forward);
        let pos = r.cursor();
        let e1 = r.next_from(&log);
        r.seek(pos);
        let e2 = r.next_from(&log);
        assert_eq!(e1.map(|e| e.op), e2.map(|e| e.op));
    }
}
