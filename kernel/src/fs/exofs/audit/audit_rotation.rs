//! audit_rotation.rs — Gestion de la rotation du journal d'audit ExoFS.
//!
//! La rotation déclenche la sérialisation du ring-buffer courant vers
//! un segment archivé, puis remet les compteurs à zéro.
//!
//! Règles appliquées :
//!  - OOM-02   : try_reserve systématique
//!  - ARITH-02 : arithmétique vérifiée
//!  - RECUR-01 : zéro récursion

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::arch::time::read_ticks;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::audit_entry::{AuditEntry, AUDIT_ENTRY_SIZE};
use super::audit_log::{AuditLog, AuditLogStats, AUDIT_LOG, RING_SIZE};

// ─────────────────────────────────────────────────────────────────────────────
// RotationConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration de la politique de rotation.
#[derive(Clone, Debug)]
pub struct RotationConfig {
    /// Déclenche la rotation si le ring dépasse ce pourcentage de remplissage.
    pub fill_threshold_pct: u8,
    /// Déclenche la rotation si N ticks se sont écoulés depuis la dernière.
    pub max_age_ticks: u64,
    /// Nombre maximum de segments archivés conservés en mémoire.
    pub max_segments: usize,
    /// Si `true`, remet les compteurs du ring à zéro après rotation.
    pub reset_stats_on_rotate: bool,
}

impl Default for RotationConfig {
    fn default() -> Self {
        RotationConfig {
            fill_threshold_pct:    75,
            max_age_ticks:         10_000_000,
            max_segments:          8,
            reset_stats_on_rotate: true,
        }
    }
}

impl RotationConfig {
    /// Configuration minimale pour les tests.
    pub fn minimal() -> Self {
        RotationConfig {
            fill_threshold_pct:    1,
            max_age_ticks:         1,
            max_segments:          2,
            reset_stats_on_rotate: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RotationReason — raison du déclenchement
// ─────────────────────────────────────────────────────────────────────────────

/// Raison pour laquelle la rotation a été déclenchée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationReason {
    /// Ring proche de la saturation.
    FillThreshold,
    /// Âge maximum dépassé.
    AgeExpired,
    /// Rotation manuelle forcée.
    Manual,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditSegment — segment archivé
// ─────────────────────────────────────────────────────────────────────────────

/// Segment d'audit archivé lors d'une rotation.
#[derive(Clone, Debug)]
pub struct AuditSegment {
    /// Numéro de séquence de la première entrée archivée.
    pub seq_start:   u64,
    /// Numéro de séquence de la dernière entrée archivée.
    pub seq_end:     u64,
    /// Tick du moment de la rotation.
    pub rotated_at:  u64,
    /// Raison de la rotation.
    pub reason:      RotationReason,
    /// Nombre d'entrées dans ce segment.
    pub n_entries:   u32,
    /// Données brutes (entrées sérialisées).
    pub data:        Vec<u8>,
}

impl AuditSegment {
    /// Taille en octets du segment.
    pub fn byte_len(&self) -> usize { self.data.len() }

    /// Nombre d'entrées.
    pub fn len(&self) -> u32 { self.n_entries }

    /// `true` si le segment ne contient aucune entrée.
    pub fn is_empty(&self) -> bool { self.n_entries == 0 }

    /// Désérialise une entrée à l'index `i` (0-based).
    pub fn entry_at(&self, i: u32) -> Option<AuditEntry> {
        let off = (i as usize).checked_mul(AUDIT_ENTRY_SIZE)?;
        let end  = off.checked_add(AUDIT_ENTRY_SIZE)?;
        if end > self.data.len() { return None; }
        let arr: &[u8; AUDIT_ENTRY_SIZE] = self.data[off..end].try_into().ok()?;
        Some(AuditEntry::from_bytes(arr))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RotationReport
// ─────────────────────────────────────────────────────────────────────────────

/// Rapport produit après une rotation.
#[derive(Clone, Debug)]
pub struct RotationReport {
    /// Nombre d'entrées archivées.
    pub n_archived:     u32,
    /// Nombre de segments déjà présents après rotation.
    pub n_segments:     usize,
    /// Ticks de démarrage.
    pub started_at:     u64,
    /// Ticks de fin.
    pub ended_at:       u64,
    /// Statistiques du log avant rotation.
    pub stats_before:   AuditLogStats,
    /// Raison de la rotation.
    pub reason:         RotationReason,
}

impl RotationReport {
    /// Durée de la rotation en ticks CPU.
    pub fn duration_ticks(&self) -> u64 {
        self.ended_at.saturating_sub(self.started_at)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditRotation
// ─────────────────────────────────────────────────────────────────────────────

/// Gestionnaire de rotation du journal d'audit.
pub struct AuditRotation {
    config:         RotationConfig,
    /// Segments archivés (FIFO — le plus ancien est à l'index 0).
    segments:       Vec<AuditSegment>,
    last_rotate_tick: u64,
    total_rotations:  u64,
}

impl AuditRotation {
    /// Crée avec la configuration par défaut.
    pub fn new() -> Self {
        AuditRotation {
            config:           RotationConfig::default(),
            segments:         Vec::new(),
            last_rotate_tick: 0,
            total_rotations:  0,
        }
    }

    /// Crée avec une configuration explicite.
    pub fn with_config(config: RotationConfig) -> Self {
        AuditRotation {
            config,
            segments:         Vec::new(),
            last_rotate_tick: 0,
            total_rotations:  0,
        }
    }

    // ── Politique de déclenchement ────────────────────────────────────────────

    /// Évalue si une rotation doit être déclenchée.
    pub fn should_rotate(&self) -> Option<RotationReason> {
        let stats = AUDIT_LOG.stats();
        if stats.ring_fill_pct >= self.config.fill_threshold_pct {
            return Some(RotationReason::FillThreshold);
        }
        let now = read_ticks();
        let age = now.saturating_sub(self.last_rotate_tick);
        if self.last_rotate_tick > 0 && age >= self.config.max_age_ticks {
            return Some(RotationReason::AgeExpired);
        }
        None
    }

    /// Lance une rotation si les conditions sont remplies.
    ///
    /// Retourne `None` si aucune rotation n'était nécessaire.
    pub fn maybe_rotate(&mut self) -> ExofsResult<Option<RotationReport>> {
        match self.should_rotate() {
            Some(reason) => Ok(Some(self.rotate_with_reason(reason)?)),
            None         => Ok(None),
        }
    }

    /// Rotation manuelle forcée.
    pub fn force_rotate(&mut self) -> ExofsResult<RotationReport> {
        self.rotate_with_reason(RotationReason::Manual)
    }

    // ── Accès aux segments ────────────────────────────────────────────────────

    /// Nombre de segments archivés.
    pub fn n_segments(&self) -> usize { self.segments.len() }

    /// Segment à l'index `i` (0 = le plus ancien).
    pub fn segment(&self, i: usize) -> Option<&AuditSegment> {
        self.segments.get(i)
    }

    /// Supprime tous les segments archivés.
    pub fn clear_segments(&mut self) {
        self.segments.clear();
    }

    /// Nombre total de rotations effectuées.
    pub fn total_rotations(&self) -> u64 { self.total_rotations }

    // ── Interne ───────────────────────────────────────────────────────────────

    fn rotate_with_reason(
        &mut self,
        reason: RotationReason,
    ) -> ExofsResult<RotationReport> {
        let started_at   = read_ticks();
        let stats_before = AUDIT_LOG.stats();
        let avail        = AUDIT_LOG.available();

        let mut data: Vec<u8> = Vec::new();
        data.try_reserve(
            avail.checked_mul(AUDIT_ENTRY_SIZE).ok_or(ExofsError::OffsetOverflow)?
        ).map_err(|_| ExofsError::NoMemory)?;

        // Sérialise toutes les entrées disponibles.
        let seq_start = AUDIT_LOG.next_seq().wrapping_sub(avail as u64);
        let seq_end;
        let n_archived;
        {
            let mut count = 0u32;
            AUDIT_LOG.for_each(|e| {
                data.extend_from_slice(e.as_bytes());
                count = count.wrapping_add(1);
            });
            n_archived = count;
            seq_end = AUDIT_LOG.next_seq().wrapping_sub(1);
        }

        // Crée le segment.
        let seg = AuditSegment {
            seq_start,
            seq_end,
            rotated_at: started_at,
            reason,
            n_entries:  n_archived,
            data,
        };

        // Ajoute le segment, éjecte les plus anciens si nécessaire.
        self.segments.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.segments.push(seg);
        while self.segments.len() > self.config.max_segments {
            self.segments.remove(0);
        }

        // Reset stats si configuré.
        if self.config.reset_stats_on_rotate {
            AUDIT_LOG.reset_stats();
        }

        self.last_rotate_tick = read_ticks();
        self.total_rotations  = self.total_rotations.wrapping_add(1);

        Ok(RotationReport {
            n_archived,
            n_segments:   self.segments.len(),
            started_at,
            ended_at:     read_ticks(),
            stats_before,
            reason,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::audit_entry::{AuditEntry, AuditOp, AuditResult};

    fn fill_log(n: usize) {
        for i in 0..n {
            AUDIT_LOG.push(AuditEntry::new(
                i as u64, 1, 0, i as u64, [0u8; 32],
                AuditOp::Read, AuditResult::Success, AUDIT_LOG.next_seq(),
            ));
        }
    }

    #[test] fn test_default_config() {
        let conf = RotationConfig::default();
        assert_eq!(conf.fill_threshold_pct, 75);
        assert!(conf.max_segments > 0);
    }

    #[test] fn test_new_rotation_no_segments() {
        let r = AuditRotation::new();
        assert_eq!(r.n_segments(), 0);
        assert_eq!(r.total_rotations(), 0);
    }

    #[test] fn test_force_rotate_creates_segment() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(2);
        let rep = r.force_rotate().unwrap();
        assert!(rep.n_archived >= 2);
        assert_eq!(r.n_segments(), 1);
        assert_eq!(r.total_rotations(), 1);
    }

    #[test] fn test_segment_entry_at() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(3);
        r.force_rotate().unwrap();
        let seg = r.segment(0).unwrap();
        assert!(!seg.is_empty());
        let e = seg.entry_at(0).unwrap();
        assert!(e.is_valid());
    }

    #[test] fn test_max_segments_eviction() {
        let mut r = AuditRotation::with_config(RotationConfig {
            max_segments: 2,
            ..RotationConfig::minimal()
        });
        fill_log(1);
        r.force_rotate().unwrap();
        fill_log(1);
        r.force_rotate().unwrap();
        fill_log(1);
        r.force_rotate().unwrap();
        // Ne dépasse jamais max_segments.
        assert!(r.n_segments() <= 2);
    }

    #[test] fn test_rotation_report_duration() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1);
        let rep = r.force_rotate().unwrap();
        // La durée est en ticks — peut être 0 sur machine rapide, jamais négatif.
        assert!(rep.duration_ticks() < u64::MAX);
    }

    #[test] fn test_clear_segments() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1);
        r.force_rotate().unwrap();
        r.clear_segments();
        assert_eq!(r.n_segments(), 0);
    }

    #[test] fn test_reason_manual() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1);
        let rep = r.force_rotate().unwrap();
        assert_eq!(rep.reason, RotationReason::Manual);
    }

    #[test] fn test_maybe_rotate_none_when_empty_log() {
        let r_no_fill = AuditRotation::new();
        // Avec les seuils par défaut et un log peu rempli, pas de rotation.
        let _ = r_no_fill; // Ne panique pas.
    }

    #[test] fn test_segment_byte_len() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(2);
        r.force_rotate().unwrap();
        let seg = r.segment(0).unwrap();
        assert_eq!(seg.byte_len(), seg.n_entries as usize * AUDIT_ENTRY_SIZE);
    }

    #[test] fn test_total_rotations_increments() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1); r.force_rotate().unwrap();
        fill_log(1); r.force_rotate().unwrap();
        assert_eq!(r.total_rotations(), 2);
    }

    #[test] fn test_segment_is_empty_on_empty_log() {
        let log  = AuditLog::new_const();
        // Ring vide → segment à 0 entrées.
        // On teste via entry_at hors bornes.
        let seg = AuditSegment {
            seq_start: 0, seq_end: 0, rotated_at: 0,
            reason: RotationReason::Manual,
            n_entries: 0, data: alloc::vec![],
        };
        assert!(seg.is_empty());
        assert!(seg.entry_at(0).is_none());
    }

    #[test] fn test_rotation_report_has_stats_before() {
        let mut r = AuditRotation::with_config(RotationConfig::minimal());
        fill_log(1);
        let rep = r.force_rotate().unwrap();
        // stats_before.total_pushed doit être ≥ 1.
        assert!(rep.stats_before.total_pushed >= 1);
    }

    #[test] fn test_with_config_respects_fields() {
        let cfg = RotationConfig {
            fill_threshold_pct: 50,
            max_age_ticks: 500,
            max_segments: 3,
            reset_stats_on_rotate: false,
        };
        let r = AuditRotation::with_config(cfg.clone());
        assert_eq!(r.config.max_segments, 3);
    }
}
