//! audit_writer.rs — Écriture d'entrées dans le journal d'audit ExoFS (no_std).
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
use super::audit_entry::{AuditEntry, AuditEntryBuilder, AuditOp, AuditResult};
use super::audit_log::AUDIT_LOG;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Capacité maximale du buffer de batch.
pub const WRITER_BATCH_MAX: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// WriterContext — contexte de l'acteur courant
// ─────────────────────────────────────────────────────────────────────────────

/// Contexte d'authentification de l'acteur qui émet les entrées.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriterContext {
    /// UID de l'acteur.
    pub actor_uid: u64,
    /// Capabilities de l'acteur (bitmask).
    pub actor_cap: u64,
}

impl WriterContext {
    pub fn new(actor_uid: u64, actor_cap: u64) -> Self {
        WriterContext { actor_uid, actor_cap }
    }

    /// Contexte noyau (UID 0, toutes capacités).
    pub fn kernel() -> Self {
        WriterContext { actor_uid: 0, actor_cap: u64::MAX }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// WritePolicy — politique d'écriture
// ─────────────────────────────────────────────────────────────────────────────

/// Politique d'écriture dans le ring-buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WritePolicy {
    /// Écriture immédiate à chaque appel.
    Immediate,
    /// Accumule dans un buffer interne — vidé via `flush()`.
    Buffered,
}

// ─────────────────────────────────────────────────────────────────────────────
// WriteStats
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques cumulées de l'`AuditWriter`.
#[derive(Clone, Debug, Default)]
pub struct WriteStats {
    /// Nombre total d'entrées écrites dans le ring.
    pub total_written:  u64,
    /// Nombre total de batches flushés.
    pub total_flushes:  u64,
    /// Nombre d'entrées retenues en erreur (buffer plein).
    pub total_dropped:  u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// AuditWriter
// ─────────────────────────────────────────────────────────────────────────────

/// Écrivain d'entrées dans le journal d'audit.
///
/// Supporte l'écriture immédiate et bufférisée.  
/// Thread-safety : à utiliser depuis un seul thread (pas de lock interne).
pub struct AuditWriter {
    ctx:    WriterContext,
    policy: WritePolicy,
    buffer: Vec<AuditEntry>,
    stats:  WriteStats,
}

impl AuditWriter {
    /// Crée un writer avec contexte et politique donnés.
    pub fn new(ctx: WriterContext, policy: WritePolicy) -> Self {
        AuditWriter {
            ctx,
            policy,
            buffer: Vec::new(),
            stats:  WriteStats::default(),
        }
    }

    /// Crée un writer immédiat pour le noyau.
    pub fn kernel_immediate() -> Self {
        Self::new(WriterContext::kernel(), WritePolicy::Immediate)
    }

    /// Change la politique d'écriture.
    pub fn set_policy(&mut self, policy: WritePolicy) {
        self.policy = policy;
    }

    // ── Écriture simple ───────────────────────────────────────────────────────

    /// Enregistre une opération simple sur un objet.
    ///
    /// Le tick est lu automatiquement.
    pub fn write(
        &mut self,
        object_id: u64,
        blob_id:   [u8; 32],
        op:        AuditOp,
        result:    AuditResult,
    ) -> ExofsResult<()> {
        let entry = AuditEntryBuilder::new()
            .tick(read_ticks())
            .actor_uid(self.ctx.actor_uid)
            .actor_cap(self.ctx.actor_cap)
            .object_id(object_id)
            .blob_id(blob_id)
            .op(op)
            .result(result)
            .seq(AUDIT_LOG.next_seq())
            .build()?;
        self.emit(entry)
    }

    /// Enregistre une entrée déjà construite.
    pub fn write_entry(&mut self, entry: AuditEntry) -> ExofsResult<()> {
        entry.validate()?;
        self.emit(entry)
    }

    // ── Raccourcis par opération ──────────────────────────────────────────────

    /// Enregistre un accès en lecture.
    pub fn record_read(&mut self, object_id: u64, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.write(object_id, blob_id, AuditOp::Read, AuditResult::Success)
    }

    /// Enregistre une écriture réussie.
    pub fn record_write(&mut self, object_id: u64, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.write(object_id, blob_id, AuditOp::Write, AuditResult::Success)
    }

    /// Enregistre une création.
    pub fn record_create(&mut self, object_id: u64, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.write(object_id, blob_id, AuditOp::Create, AuditResult::Success)
    }

    /// Enregistre une suppression.
    pub fn record_delete(&mut self, object_id: u64, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.write(object_id, blob_id, AuditOp::Delete, AuditResult::Success)
    }

    /// Enregistre un refus de permission.
    pub fn record_denied(
        &mut self,
        object_id: u64,
        blob_id:   [u8; 32],
        op:        AuditOp,
    ) -> ExofsResult<()> {
        self.write(object_id, blob_id, op, AuditResult::Denied)
    }

    /// Enregistre un échec de checksum.
    pub fn record_checksum_fail(&mut self, object_id: u64) -> ExofsResult<()> {
        self.write(object_id, [0u8; 32], AuditOp::ChecksumFail, AuditResult::Error)
    }

    /// Enregistre un événement de clé cryptographique.
    pub fn record_crypto_key(
        &mut self,
        object_id: u64,
        result:    AuditResult,
    ) -> ExofsResult<()> {
        self.write(object_id, [0u8; 32], AuditOp::CryptoKey, result)
    }

    /// Enregistre un changement de politique.
    pub fn record_policy_change(&mut self, object_id: u64) -> ExofsResult<()> {
        self.write(object_id, [0u8; 32], AuditOp::PolicyChange, AuditResult::Success)
    }

    // ── Batch ─────────────────────────────────────────────────────────────────

    /// Écrit un lot d'entrées (max `WRITER_BATCH_MAX`).
    ///
    /// Toutes les entrées sont validées avant écriture. Si une entrée est
    /// invalide, le batch est annulé et `InvalidArgument` est retourné.
    pub fn write_batch(&mut self, entries: &[AuditEntry]) -> ExofsResult<u32> {
        if entries.len() > WRITER_BATCH_MAX {
            return Err(ExofsError::InvalidArgument);
        }
        for e in entries {
            e.validate()?;
        }
        let mut written = 0u32;
        for &e in entries {
            self.emit(e)?;
            written = written.checked_add(1).ok_or(ExofsError::OffsetOverflow)?;
        }
        Ok(written)
    }

    // ── Flush / buffer ────────────────────────────────────────────────────────

    /// Vide le buffer interne dans le ring-buffer.
    ///
    /// Retourne le nombre d'entrées flushées.
    pub fn flush(&mut self) -> u32 {
        let n = self.buffer.len();
        for entry in self.buffer.drain(..) {
            AUDIT_LOG.push(entry);
            self.stats.total_written = self.stats.total_written.wrapping_add(1);
        }
        self.stats.total_flushes = self.stats.total_flushes.wrapping_add(1);
        n as u32
    }

    /// Nombre d'entrées en attente dans le buffer.
    pub fn pending(&self) -> usize { self.buffer.len() }

    /// `true` si le buffer interne est vide.
    pub fn is_flushed(&self) -> bool { self.buffer.is_empty() }

    // ── Statistiques ──────────────────────────────────────────────────────────

    /// Retourne les statistiques courantes.
    pub fn stats(&self) -> &WriteStats { &self.stats }

    /// Remet les statistiques à zéro.
    pub fn reset_stats(&mut self) { self.stats = WriteStats::default(); }

    // ── Interne ───────────────────────────────────────────────────────────────

    fn emit(&mut self, entry: AuditEntry) -> ExofsResult<()> {
        match self.policy {
            WritePolicy::Immediate => {
                AUDIT_LOG.push(entry);
                self.stats.total_written = self.stats.total_written.wrapping_add(1);
            }
            WritePolicy::Buffered => {
                if self.buffer.len() >= WRITER_BATCH_MAX {
                    // Buffer plein : flush automatique.
                    self.flush();
                }
                self.buffer.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                self.buffer.push(entry);
            }
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers libres
// ─────────────────────────────────────────────────────────────────────────────

/// Écriture rapide d'une entrée de sécurité depuis n'importe où.
pub fn write_security_event(
    actor_uid: u64,
    object_id: u64,
    op:        AuditOp,
    result:    AuditResult,
) {
    let tick = read_ticks();
    if let Ok(entry) = AuditEntryBuilder::new()
        .tick(tick)
        .actor_uid(actor_uid)
        .actor_cap(0)
        .object_id(object_id)
        .op(op)
        .result(result)
        .seq(AUDIT_LOG.next_seq())
        .build()
    {
        AUDIT_LOG.push(entry);
    }
}

/// Enregistre immédiatement un refus de permission (inline, pas d'alloc).
pub fn record_perm_denied(actor_uid: u64, object_id: u64, op: AuditOp) {
    write_security_event(actor_uid, object_id, op, AuditResult::Denied);
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::audit_log::AuditLog;

    fn writer() -> AuditWriter {
        AuditWriter::new(WriterContext::new(42, 0xFF), WritePolicy::Immediate)
    }

    #[test] fn test_write_read_is_ok() {
        let mut w = writer();
        w.write(1, [0u8; 32], AuditOp::Read, AuditResult::Success).unwrap();
        let e = AUDIT_LOG.read_last().unwrap();
        assert!(e.is_valid());
    }

    #[test] fn test_record_create() {
        let mut w = writer();
        w.record_create(99, [1u8; 32]).unwrap();
        let e = AUDIT_LOG.read_last().unwrap();
        assert_eq!(e.op, AuditOp::Create as u8);
    }

    #[test] fn test_record_denied() {
        let mut w = writer();
        w.record_denied(1, [0u8; 32], AuditOp::Write).unwrap();
        let e = AUDIT_LOG.read_last().unwrap();
        assert_eq!(e.result, AuditResult::Denied as u8);
    }

    #[test] fn test_stats_increments() {
        let mut w = writer();
        w.record_read(1, [0u8; 32]).unwrap();
        w.record_write(2, [0u8; 32]).unwrap();
        assert_eq!(w.stats().total_written, 2);
    }

    #[test] fn test_buffered_flush() {
        let mut w = AuditWriter::new(WriterContext::kernel(), WritePolicy::Buffered);
        w.record_read(10, [0u8; 32]).unwrap();
        assert_eq!(w.pending(), 1);
        let n = w.flush();
        assert_eq!(n, 1);
        assert!(w.is_flushed());
    }

    #[test] fn test_batch_valid() {
        let mut w = writer();
        let entries: Vec<AuditEntry> = (0u64..4).map(|i| {
            AuditEntry::new(i, 1, 0, i, [0u8; 32], AuditOp::Read, AuditResult::Success, i)
        }).collect();
        let n = w.write_batch(&entries).unwrap();
        assert_eq!(n, 4);
    }

    #[test] fn test_batch_too_large() {
        let mut w = writer();
        let entries: Vec<AuditEntry> = (0u64..300).map(|i| {
            AuditEntry::new(i, 1, 0, i, [0u8; 32], AuditOp::Read, AuditResult::Success, i)
        }).collect();
        assert!(w.write_batch(&entries).is_err());
    }

    #[test] fn test_write_entry_invalid_magic() {
        let mut w = writer();
        let mut e = AuditEntry::new(1, 1, 0, 1, [0u8; 32],
            AuditOp::Read, AuditResult::Success, 0);
        e.magic = 0xDEAD;
        assert!(w.write_entry(e).is_err());
    }

    #[test] fn test_kernel_writer() {
        let mut w = AuditWriter::kernel_immediate();
        w.record_crypto_key(0, AuditResult::Success).unwrap();
        let e = AUDIT_LOG.read_last().unwrap();
        assert_eq!(e.op, AuditOp::CryptoKey as u8);
    }

    #[test] fn test_write_policy_change() {
        let mut w = writer();
        w.record_policy_change(5).unwrap();
        let e = AUDIT_LOG.read_last().unwrap();
        assert!(e.is_security());
    }

    #[test] fn test_reset_stats() {
        let mut w = writer();
        w.record_read(1, [0u8; 32]).unwrap();
        w.reset_stats();
        assert_eq!(w.stats().total_written, 0);
    }

    #[test] fn test_write_checksum_fail() {
        let mut w = writer();
        w.record_checksum_fail(77).unwrap();
        let e = AUDIT_LOG.read_last().unwrap();
        assert_eq!(e.op, AuditOp::ChecksumFail as u8);
        assert_eq!(e.result, AuditResult::Error as u8);
    }

    #[test] fn test_buffered_no_flush_until_explicit() {
        let mut w = AuditWriter::new(WriterContext::kernel(), WritePolicy::Buffered);
        w.record_delete(10, [0u8; 32]).unwrap();
        assert_eq!(w.pending(), 1);
        assert!(!w.is_flushed());
    }

    #[test] fn test_write_entry_valid_roundtrip() {
        let mut w = writer();
        let e = AuditEntry::new(50, 7, 0, 3, [0u8; 32],
            AuditOp::Rename, AuditResult::Success, 0);
        w.write_entry(e).unwrap();
        let last = AUDIT_LOG.read_last().unwrap();
        assert_eq!(last.op, AuditOp::Rename as u8);
    }
}
